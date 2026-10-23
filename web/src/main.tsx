import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import { LocationProvider, Router, Route } from "preact-iso";
import { BoardWide } from "./BoardWide";
import { BoardPhone } from "./BoardPhone";
import { PlansView } from "./PlansView";
import {
  snapshot,
  plans,
  connection,
  error,
  present,
  selectJob,
  start,
} from "./state";
import "@fontsource/ibm-plex-sans/latin-400.css";
import "@fontsource/ibm-plex-sans/latin-500.css";
import "@fontsource/ibm-plex-sans/latin-600.css";
import "@fontsource/ibm-plex-mono/latin-400.css";
import "@fontsource/ibm-plex-mono/latin-500.css";
import "./style.css";

function Board({ id }: { id?: string }) {
  const snap = snapshot.value;
  const [narrow, setNarrow] = useState(matchMedia("(max-width:699px)").matches);
  const picked = id
    ? Number(id)
    : (snap?.jobs.find((j) => j.state === "working")?.id ?? snap?.jobs[0]?.id);
  useEffect(() => {
    const query = matchMedia("(max-width:699px)");
    const change = () => setNarrow(query.matches);
    query.addEventListener("change", change);
    return () => query.removeEventListener("change", change);
  }, []);
  useEffect(() => {
    if (picked) void selectJob(picked);
  }, [picked]);
  if (!snap) return <Status />;
  if (!snap.jobs.length)
    return (
      <div class="empty">
        <a href="/plans">Plans</a>
        <h1>No jobs yet</h1>
        <p>Jobs started from Forkdeck appear here live.</p>
      </div>
    );
  const data = snap.jobs.map(present);
  if (narrow && !id) {
    return <BoardPhone data={data} live={connection.value} />;
  }
  return (
    <div class={narrow ? "mobile-detail" : ""}>
      <BoardWide
        data={data}
        picked={picked}
        automation={snap.automation}
        agents={snap.agents}
        busy={snap.busy}
        capacity={snap.capacity}
        workspace={snap.workspace}
        router={`${snap.router.kind} · ${snap.router.calls} routes · ${snap.router.last_ms} ms`}
        live={connection.value}
      />
    </div>
  );
}
function Plans({ id }: { id?: string }) {
  const all = plans.value;
  if (!snapshot.value) return <Status />;
  if (!all.length)
    return (
      <div class="empty">
        <a href="/">← Board</a>
        <h1>Issue plans</h1>
        <p>No issue plans yet.</p>
      </div>
    );
  const plan = id ? all.find((p) => String(p.id) === id) : all[0];
  if (!plan)
    return (
      <div class="empty">
        <a href="/plans">Plans</a>
        <h1>Plan not found</h1>
      </div>
    );
  return <PlansView plan={plan} allPlans={all} />;
}
function Status() {
  return (
    <div class="empty" role="status">
      <h1>forkdeck</h1>
      <p>{error.value || "Connecting…"}</p>
    </div>
  );
}
function App() {
  useEffect(start, []);
  return (
    <LocationProvider>
      <Router>
        <Route path="/" component={Board} />
        <Route path="/jobs/:id" component={Board} />
        <Route path="/plans" component={Plans} />
        <Route path="/plans/:id" component={Plans} />
        <Route default component={Status} />
      </Router>
    </LocationProvider>
  );
}
render(<App />, document.getElementById("app")!);
