import { Composer, Intake } from "./Actions";
import { Account } from "./Account";
import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import { LocationProvider, Router, Route, useLocation } from "preact-iso";
import { BoardWide } from "./BoardWide";
import { BoardPhone } from "./BoardPhone";
import { Operational, Navigation } from "./Operational";
import { PlansView } from "./PlansView";
import { plans, connection, error, selectJob, loadIdentity } from "./state";
import { board, connect } from "./live";
import { present } from "./ui";
import { RepoPicker } from "./RepoPicker";
import "@fontsource/ibm-plex-sans/latin-400.css";
import "@fontsource/ibm-plex-sans/latin-500.css";
import "@fontsource/ibm-plex-sans/latin-600.css";
import "@fontsource/ibm-plex-mono/latin-400.css";
import "@fontsource/ibm-plex-mono/latin-500.css";
import "./style.css";

function Board({ id }: { id?: string }) {
  const current = board.value;
  const snap = current?.snapshot;
  const [narrow, setNarrow] = useState(matchMedia("(max-width:699px)").matches);
  const picked = id
    ? Number(id)
    : (current?.jobs.find((j) => j.state() === "working")?.id() ??
      current?.jobs[0]?.id());
  useEffect(() => {
    const query = matchMedia("(max-width:699px)");
    const change = () => setNarrow(query.matches);
    query.addEventListener("change", change);
    return () => query.removeEventListener("change", change);
  }, []);
  useEffect(() => {
    if (picked) void selectJob(picked);
  }, [picked]);
  if (!snap || !current) return <Status />;
  if (!current.jobs.length)
    return (
      <div class="empty">
        <header class="app-header">
          <Navigation />
          <span class="grow" />
          <Account />
        </header>
        <h1>No jobs yet</h1>
        <p>Jobs started from Forkdeck appear here live.</p>
        <Intake /><Composer />
      </div>
    );
  const data = current.jobs.map(present);
  const lanes = Array.from(
    { length: snap.lanesLength() }, (_, i) => snap.lanes(i)!,
  );
  if (narrow && !id) {
    return <BoardPhone data={data} live={connection.value} lanes={lanes} />;
  }
  return (
    <div class={narrow ? "mobile-detail" : ""}>
      <BoardWide
        data={data}
        picked={picked}
        open={!!id}
        automation={current.automation}
        agents={snap.agents()}
        busy={snap.busy()}
        capacity={snap.capacity()}
        workspace={snap.workspace() ?? ""}
        router={`${snap.router()?.kind() ?? ""} · ${snap.router()?.calls() ?? 0} routes · ${snap.router()?.lastMs() ?? 0} ms`}
        live={connection.value}
        lanes={lanes}
      />
    </div>
  );
}
function Plans({ id }: { id?: string }) {
  const all = plans.value;
  if (!board.value) return <Status />;
  if (!all.length)
    return (
      <div class="empty">
        <header class="app-header">
          <Navigation />
          <span class="grow" />
          <Account />
        </header>
        <h1>Issue plans</h1>
        <p>No issue plans yet.</p>
        <Composer issuePlan />
      </div>
    );
  const plan = id ? all.find((p) => String(p.id) === id) : all[0];
  if (!plan)
    return (
      <div class="empty">
        <header class="app-header">
          <Navigation />
          <span class="grow" />
          <Account />
        </header>
        <h1>Plan not found</h1>
      </div>
    );
  return <PlansView plan={plan} allPlans={all} />;
}
/** The page was called Doctor & settings; old links land on its new address. */
function MovedToConfiguration() {
  const { route } = useLocation();
  useEffect(() => route("/configuration", true), []);
  return null;
}
function Status() {
  return (
    <div class="empty" role="status">
      <Account />
      <h1>forkdeck</h1>
      <p>{error.value || "Connecting…"}</p>
    </div>
  );
}
function App() {
  useEffect(() => {
    loadIdentity();
    return connect();
  }, []);
  return (
    <LocationProvider>
      <Router>
        <Route path="/" component={Board} />
        <Route path="/jobs/:id" component={Board} />
        <Route path="/plans" component={Plans} />
        <Route path="/plans/:id" component={Plans} />
        <Route path="/repos" component={() => <Operational page="repos" />} />
        <Route path="/repos/add/:id" component={RepoPicker} />
        <Route
          path="/deploys"
          component={() => <Operational page="deploys" />}
        />
        <Route path="/stats" component={() => <Operational page="stats" />} />
        <Route path="/configuration" component={() => <Operational page="doctor" />} />
        <Route path="/doctor" component={MovedToConfiguration} />
        <Route default component={Status} />
      </Router>
    </LocationProvider>
  );
}
render(<App />, document.getElementById("app")!);
