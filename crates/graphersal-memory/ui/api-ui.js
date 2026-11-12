// The `/api/ui/*` endpoints that `graphersal-memory ui` answers itself (the profile API does not
// know them): status, projects, start context, the start context budget. The token header is
// added by ui/boot.js to every request of the page to its own `/api/`.

async function call(path, options = {}) {
  let r;
  try {
    r = await fetch(path, { cache: 'no-store', ...options });
  } catch (err) {
    throw new Error('The memory UI is not running any more. Start it again: graphersal-memory ui');
  }
  const text = await r.text();
  let data = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch (err) {
    throw new Error(`The UI server sent no JSON (HTTP ${r.status}).`);
  }
  if (!r.ok) throw new Error((data && data.error) || `HTTP ${r.status}`);
  return data;
}

/** The daemon, its indexing and the status/doctor lines. */
export function getStatus() {
  return call('/api/ui/status');
}

/** The projects of projects.json: [{id, name, paths, remotes}]. */
export async function getProjects() {
  const answer = await call('/api/ui/projects');
  return (answer && answer.projects) || [];
}

/** The start context of a project id ("" = the global memories only), with its entries. */
export function getStartContext(project) {
  return call(`/api/ui/start-context?project=${encodeURIComponent(project || '')}`);
}

/** The start context budget. */
export function getSettings() {
  return call('/api/ui/settings');
}

/** Sets the start context budget (characters; 0 = no start context). */
export function setBudget(chars) {
  return call('/api/ui/settings', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ start_context_chars: chars }),
  });
}
