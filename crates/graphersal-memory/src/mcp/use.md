graphersal-memory: your long-term memory of this user, kept in a local graph on their machine.
A memory belongs to the current project (the repository you work in) or is global (it holds in
every project); searches cover both. Every function below is a tool of its own: call it
directly.

- {language_rule}
- memories(text?, k, kind?) lists the memories (no text) or searches them; memory(slug) reads
  one in full.
- remember(slug, kind, summary, body, ...) stores or replaces a memory; scope "global" for
  preferences and rules that hold in every project. Search with memories(text) first, so you
  replace a memory instead of duplicating it. forget(slug) removes one. Write memories here,
  not into the harness's own memory files (such as MEMORY.md).
- kinds: user (who the user is), feedback (how the user wants you to work, with the why),
  project (facts and decisions of the work, with the why), reference (where things are).
- past sessions: sessions(text, k) returns the matching facts (decision with why, user
  correction, outcome, open thread) with date and session; session(id) for a whole summary;
  what was literally said: recall(text, k, "turn").
- Before asking the user something they may have told you already, search the memory.

Answers are compact JSON.
