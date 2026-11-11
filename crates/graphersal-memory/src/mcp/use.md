graphersal-memory: your long-term memory of this user, kept in a local graph on their machine.
A memory belongs to the current project (the repository you work in) or is global (it holds in
every project); searches cover both. Every function below is a tool of its own: call it
directly.

- {language_rule}
- memories(text?, k, kind?) lists the memories (no text) or searches them; memory(slug) reads
  one in full.
- remember(slug, kind, summary, body, ...) stores or replaces a memory; scope "global" for
  preferences and rules that hold in every project. forget(slug) removes one. Write memories
  here, not into the harness's own memory files (such as MEMORY.md).
- remember answers the nearest existing memories with a check: settle each as it says
  (supersedes: [old slug] for a later decision; settle(slug, decision, other)). A memory
  marked SUPERSEDED was replaced by the one named; conflicts() lists open conflicts and
  unchecked imported memories: ask the user which memory holds, then settle it.
- pin(slug) when the user wants a memory always in mind: pinned, feedback and user memories
  are loaded into the start of every session (shown below when there are any).
- kinds: user (who the user is), feedback (how the user wants you to work, with the why),
  project (facts and decisions of the work, with the why), reference (where things are).
- past sessions: sessions(text, k) returns the matching facts (decision with why, user
  correction, outcome, open thread) with date and session; session(id) for a whole summary;
  what was literally said: recall(text, k, "turn").
- Before asking the user something they may have told you already, search the memory.

Answers are compact JSON.
