import { principal } from "./state";

export function Account() {
  const user = principal.value;
  if (!user) return null;
  return (
    <div class="signed-in">
      <span title={`${user.subject} · ${user.role}`}>{user.name}</span>
      <form method="post" action="/auth/logout">
        <button class="intake" type="submit">Sign out</button>
      </form>
    </div>
  );
}
