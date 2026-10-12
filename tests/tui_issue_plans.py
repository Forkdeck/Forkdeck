#!/usr/bin/env python3
"""Offline forge + PTY acceptance tests. Usage: python3 tests/tui_issue_plans.py BINARY."""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

BINARY = str(Path(sys.argv[1]).resolve())
F6 = b"\x1b[17~"
FAKE = r'''#!/usr/bin/python3
import json, os, pathlib, sys, time
root = pathlib.Path(os.environ['FORKDECK_TEST_ROOT'])
args = sys.argv[1:]
if '--version' in args:
 print('offline fixture'); sys.exit(0)
def log(name, obj):
 with (root / name).open('a') as f: f.write(json.dumps(obj) + '\n')
if pathlib.Path(sys.argv[0]).name == 'planner':
 prompt = sys.stdin.read()
 assert '--permission-mode' in args and args[args.index('--permission-mode')+1] == 'plan', args
 review = 'Review this implementation PLAN' in prompt
 log('agents.jsonl', {'review': review, 'prompt': prompt, 'args': args})
 if (root / 'slow').exists():
  (root / 'running').touch()
  while (root / 'slow').exists(): time.sleep(.03)
 if review:
  result = 'Acceptance criteria, dependencies and tests checked.\n' + ('詳細 ' * 250) + '\nREVIEW_TAIL\nVERDICT: ' + ('FAIL' if (root/'reject').exists() else 'PASS')
 else:
  result = 'Goal: implement the request.\nSteps: inspect, implement, test.\nDependencies: existing library.\nAcceptance criteria: requested behavior passes.\nTests: unit and integration.\n' + ('日本語 ' * 250) + '\nPLAN_TAIL\nPLAN: READY'
 print(json.dumps({'type':'result', 'is_error':False, 'result':result})); sys.exit(0)
assert args[0] == 'api', args
method = 'GET'; path = None; i = 1
while i < len(args):
 if args[i] in ('-X', '--method'): method=args[i+1]; i+=2
 elif args[i] == '--input': i+=2
 elif args[i] == '--paginate': i+=1
 else: path=args[i]; i+=1
data=json.loads(sys.stdin.read()) if '--input' in args else None
log('forge.jsonl', {'method':method, 'path':path, 'data':data})
dbpath=root/'forge.json'
db=json.loads(dbpath.read_text())
def save():
 temp=dbpath.with_suffix('.tmp'); temp.write_text(json.dumps(db)); temp.replace(dbpath)
def lose(kind):
 flag=root/('lose_'+kind)
 if flag.exists(): flag.unlink(); print('connection lost after request', file=sys.stderr); sys.exit(1)
def body(issue, value): issue['body']=value; issue['description']=value
is_comments=path.split('?')[0].endswith(('/comments','/notes'))
is_collection=path.split('?')[0].endswith('/issues')
issue=db.get('issue')
if method != 'GET':
 snapshots=list((root/'state/forkdeck/sessions').glob('*.json'))
 assert snapshots, 'remote mutation without durable session'
 artifact=json.loads(snapshots[0].read_text())['jobs'][0]['job']['issue_plan']
 assert artifact is not None
 if method=='POST' and is_collection: assert artifact['pending']['create_attempted']
 if method=='POST' and is_comments: assert artifact['pending']['comment_attempted']
if method=='GET':
 if (root/'fail_read').exists():
  (root/'fail_read').unlink(); print('offline', file=sys.stderr); sys.exit(1)
 if is_comments: print(json.dumps(db['comments']))
 elif is_collection: print(json.dumps([issue] if issue else []))
 elif '/members/' in path: print('{"access_level":40}')
 else:
  assert issue, 'missing issue'; print(json.dumps(issue))
elif method=='POST' and is_collection:
 assert issue is None, 'duplicate create'
 issue={'number':7,'iid':7,'title':data['title'],'state':'open','labels':[],
        'html_url':'https://github.test/o/r/issues/7','web_url':'https://gitlab.test/o/r/-/issues/7'}
 body(issue, data.get('body',data.get('description')))
 db['issue']=issue; db['creates']+=1; save(); lose('create'); print(json.dumps(issue))
elif method=='POST' and is_comments:
 db['comments'].append({'id':len(db['comments'])+1,'body':data['body'],'user':{'login':'tester'},'author':{'username':'tester','id':1},'author_association':'OWNER','updated_at':'2026-10-12T12:00:00Z'})
 db['posts']+=1; save(); lose('comment'); print(json.dumps(db['comments'][-1]))
elif path.endswith('/labels') and method=='POST':
 issue['labels']=list(set(issue['labels']+data['labels'])); save(); lose('ready'); print('[]')
elif '/labels/' in path and method=='DELETE':
 issue['labels']=[x for x in issue['labels'] if x!='agent:ready']; save(); print('{}')
elif method in ('PUT','PATCH'):
 if 'body' in data or 'description' in data: body(issue,data.get('body',data.get('description')))
 if 'add_labels' in data: issue['labels']=list(set(issue['labels']+[data['add_labels']]))
 if 'remove_labels' in data: issue['labels']=[x for x in issue['labels'] if x!=data['remove_labels']]
 save(); lose('update' if 'body' in data or 'description' in data else 'ready'); print(json.dumps(issue))
else: raise AssertionError((method,path,data))
'''


class Rig:
    def __init__(self, root, provider, existing=False, width=110):
        self.root, self.provider, self.width = root, provider, width
        root.mkdir()
        (root / "bin").mkdir()
        for name in ("gh", "glab", "planner"):
            path = root / "bin" / name
            path.write_text(FAKE)
            path.chmod(0o755)
        issue = None
        if existing:
            issue = dict(number=7, iid=7, title="Existing task", state="open", labels=["agent:ready", "human-label"], body="Human description", description="Human description", html_url="https://github.test/o/r/issues/7", web_url="https://gitlab.test/o/r/-/issues/7")
        self.write_db(dict(issue=issue, comments=[], creates=0, posts=0))
        self.config = root / "config.toml"
        self.config.write_text(f'''[router]
kind="rules"
[agents.claude]
cmd="{root}/bin/planner"
[agents.codex]
cmd="{root}/missing"
[agents.copilot]
cmd="{root}/missing"
[automation]
enabled=false
[review]
checks=["touch CHECKS_MUST_NOT_RUN"]
[[repos]]
provider="{provider}"
repo="o/r"
path="{root}"
checks=["touch CHECKS_MUST_NOT_RUN"]
''')
        self.process = None

    def db(self):
        return json.loads((self.root / "forge.json").read_text())

    def write_db(self, db):
        (self.root / "forge.json").write_text(json.dumps(db))

    def records(self, name):
        path = self.root / name
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def job(self):
        paths = list((self.root / "state/forkdeck/sessions").glob("*.json"))
        jobs = json.loads(paths[0].read_text())["jobs"] if paths else []
        return jobs[0]["job"] if jobs else {}

    def plan(self):
        return self.job().get("issue_plan") or {}

    def send(self, value):
        os.write(self.master, value)

    def wait(self, predicate, timeout=15):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if predicate():
                return
            if select.select([self.master], [], [], .03)[0]:
                self.output.extend(os.read(self.master, 65536))
        raise AssertionError(f"{self.root.name}: timeout; process={self.process.poll()} job={self.job()}\n{self.output.decode(errors='replace')[-2000:]}")

    def start(self):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 36, self.width, 0, 0))
        def terminal_session():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        self.process = subprocess.Popen([BINARY, "--config", str(self.config)], cwd=self.root,
            env={**os.environ, "TERM":"xterm-256color", "PATH":f"{self.root}/bin:"+os.environ['PATH'], "XDG_STATE_HOME":str(self.root/'state'), "FORKDECK_TEST_ROOT":str(self.root)},
            stdin=self.slave, stdout=self.slave, stderr=self.slave, preexec_fn=terminal_session)
        self.output = bytearray()
        self.wait(lambda: b"Quick task" in self.output or b"F6 issue plan" in self.output)

    def stop(self):
        self.send(b"\x03")
        self.wait(lambda: self.process.poll() is not None, timeout=5)
        mask = termios.ICANON | termios.ECHO | termios.ISIG
        assert termios.tcgetattr(self.slave)[3] & mask == self.original[3] & mask
        os.close(self.master)
        os.close(self.slave)
        self.process = None

    def begin(self, link=""):
        self.send(F6 + b"\t" + link.encode() + b"\tPlan the implementation\r")

    def reviewed(self, revision):
        self.wait(lambda: self.job().get('status') == 'Done' and self.plan().get('reviewed') == revision)

    def publish(self, revision):
        self.send(b"1\r")
        self.wait(lambda: self.job().get('status') == 'Done' and self.plan().get('published') == revision)

    def failed(self):
        self.wait(lambda: self.job().get('status') == 'Failed')

    def refine(self, text="Include edge cases"):
        self.send(b"2\r" + text.encode() + b"\r")

    def cleanup(self):
        if self.process is not None:
            self.process.kill()
            self.process.wait()
            os.close(self.master)
            os.close(self.slave)


with tempfile.TemporaryDirectory(prefix="forkdeck-plan-tests-") as directory:
    root = Path(directory)
    for provider in ("github", "gitlab"):
        rig = Rig(root/provider, provider, width=72 if provider=='gitlab' else 140)
        try:
            rig.start(); rig.begin(); rig.reviewed(1)
            assert rig.db()['creates'] == 0
            assert not rig.records('forge.jsonl'), 'new draft touched the forge before acceptance'
            rig.send(b"\x1b[F")
            rig.wait(lambda: b'PLAN_TAIL' in rig.output)
            rig.send(b"\t\x1b[F")
            rig.wait(lambda: b'REVIEW_TAIL' in rig.output)
            rig.publish(1)
            assert rig.db()['creates'] == 1 and rig.db()['posts'] == 1
            assert 'agent:ready' not in rig.db()['issue']['labels']
            rig.send(b'3\r')
            rig.wait(lambda: rig.plan().get('ready') and rig.job().get('status') == 'Done')
            assert 'intake paused' in rig.job()['outcome']
            count = len(rig.records('agents.jsonl'))
            rig.stop(); rig.start()
            assert len(rig.records('agents.jsonl')) == count, 'restart launched an agent'
            assert rig.plan()['published'] == 1
            db=rig.db()
            for field in ('body','description'): db['issue'][field]='Human intro\n'+db['issue'][field]+'\nHuman tail'
            rig.write_db(db)
            rig.send(F6); rig.refine(); rig.reviewed(2)
            assert 'agent:ready' not in rig.db()['issue']['labels']
            rig.send(F6); rig.publish(2)
            assert rig.db()['creates'] == 1 and rig.db()['posts'] == 2
            assert rig.db()['issue']['body'].startswith('Human intro\n') and rig.db()['issue']['body'].endswith('\nHuman tail')
            assert not (rig.root/'CHECKS_MUST_NOT_RUN').exists()
            rig.stop()
            print(f'PASS {provider}: create, review, accept, readiness, full text, narrow UI, restart and refinement')
        finally: rig.cleanup()

    for failure in ('create', 'comment', 'update'):
        rig = Rig(root/failure, 'github', existing=failure=='update')
        try:
            rig.start(); rig.begin('7' if failure=='update' else ''); rig.reviewed(1)
            (rig.root/('lose_'+failure)).touch()
            rig.send(b'1\r'); rig.failed()
            assert rig.plan()['pending'] is not None
            rig.stop(); rig.start(); rig.send(F6); rig.publish(1)
            assert rig.db()['creates'] == (0 if failure=='update' else 1)
            assert rig.db()['posts'] == 1
            rig.stop()
            print(f'PASS uncertain {failure}: restart reconciliation without duplicate writes')
        finally: rig.cleanup()

    rig = Rig(root/'conflict', 'gitlab', existing=True)
    try:
        rig.start(); rig.begin('https://gitlab.test/o/r/-/issues/7'); rig.reviewed(1)
        db=rig.db()
        for field in ('body','description'): db['issue'][field]+='\n<!-- forkdeck:plan:start -->\nHuman plan edit\n<!-- forkdeck:plan:end -->'
        rig.write_db(db)
        rig.send(b'1\r'); rig.failed()
        assert 'changed remotely' in rig.job()['outcome']
        assert 'Human plan edit' in rig.db()['issue']['description'] and rig.db()['posts']==0
        rig.refine('Incorporate the remote plan'); rig.reviewed(2)
        assert 'Human plan edit' in rig.records('agents.jsonl')[-2]['prompt']
        rig.send(F6); rig.publish(2)
        db=rig.db(); db['issue']['labels'].append('agent:working'); rig.write_db(db)
        rig.refine('Change the scope'); rig.failed()
        assert 'already claimed' in rig.job()['outcome']
        rig.stop()
        print('PASS conflicts preserve remote edits; refinement refreshes context; claimed issue is refused')
    finally: rig.cleanup()

    rig = Rig(root/'review-rejected', 'github')
    try:
        (rig.root/'reject').touch()
        rig.start(); rig.begin()
        rig.wait(lambda: rig.job().get('status')=='Done' and rig.job().get('verdict') is False)
        rig.send(b'1\r')
        rig.wait(lambda: b'passing plan review' in rig.output)
        assert rig.db()['creates']==0
        (rig.root/'reject').unlink()
        rig.refine('Address the review findings'); rig.reviewed(2)
        assert 'VERDICT: FAIL' in rig.records('agents.jsonl')[-2]['prompt']
        rig.send(F6); rig.publish(2); rig.stop()
        print('PASS failed plan review blocks publication; next revision includes reviewer feedback')
    finally: rig.cleanup()

    rig = Rig(root/'save-failed', 'github')
    try:
        rig.start(); rig.begin(); rig.reviewed(1)
        snapshot=next((rig.root/'state/forkdeck/sessions').glob('*.json'))
        obstacle=snapshot.with_suffix('.tmp'); obstacle.mkdir()
        rig.send(b'1\r')
        rig.wait(lambda: b'Cannot save issue plan' in rig.output)
        assert not rig.records('forge.jsonl'), 'publication started without a durable save'
        obstacle.rmdir(); rig.failed(); rig.publish(1); rig.stop()
        print('PASS a session save failure blocks publication and can be retried after repair')
    finally: rig.cleanup()

    rig = Rig(root/'ready-retry', 'gitlab')
    try:
        rig.start(); rig.begin(); rig.reviewed(1); rig.publish(1)
        (rig.root/'lose_ready').touch(); rig.send(b'3\r'); rig.failed()
        assert 'agent:ready' in rig.db()['issue']['labels']
        rig.stop(); rig.start(); rig.send(F6+b'3\r')
        rig.wait(lambda: rig.plan().get('ready') and rig.job().get('status')=='Done')
        writes=[r for r in rig.records('forge.jsonl') if (r.get('data') or {}).get('add_labels')]
        assert len(writes)==1
        rig.stop()
        print('PASS uncertain readiness reconciles the existing label after restart')
    finally: rig.cleanup()

    rig = Rig(root/'queue', 'github')
    try:
        (rig.root/'slow').touch()
        rig.start(); rig.begin()
        rig.wait(lambda: (rig.root/'running').exists())
        rig.send(F6 + b'Queued feedback\r')
        rig.wait(lambda: len(rig.job().get('queued_prompts',[])) == 1)
        (rig.root/'slow').unlink()
        rig.reviewed(2)
        assert len(rig.records('agents.jsonl')) == 4
        assert 'Queued feedback' in rig.records('agents.jsonl')[2]['prompt']
        assert rig.db()['creates'] == 0
        rig.send(F6); rig.publish(2)
        rig.stop()
        print('PASS queued feedback stays on one task, invalidates approval and receives its own review')
    finally: rig.cleanup()
