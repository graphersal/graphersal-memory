graphersal-memory: your long-term memory of this user, kept in a local graph on their machine.
A memory belongs to the current project (the repository you work in) or is global (it holds in
every project); searches cover both. Every function below is a tool of its own: call it
directly.

- {language_rule}
- memories(text?, k, kind?) lists the memories (no text) or searches them; memory(slug) reads
  one in full.
- remember(slug, kind, summary, body, ...) stores or replaces a memory: what later sessions and
  similar tasks need, not the current task's plan or progress; scope "global" for rules that
  hold in every project. forget(slug) removes one. Write memories here, not into MEMORY.md
  or other files.
- remember answers the nearest existing memories with a check: settle each as it says
  (supersedes: [old slug] for a later decision; settle(slug, decision, other)). A memory
  marked SUPERSEDED was replaced by the one named; conflicts() lists open conflicts and
  unchecked imported memories (and memories near a line of an instruction file: settle(slug,
  decision, rule: id)): ask the user which holds, then settle it; never edit an instruction file.
- pin(slug) when the user wants a memory always in mind: pinned, feedback and user memories
  start every session (shown below).
- kinds: user (who the user is), feedback (how the user wants you to work, with the why),
  project (lasting facts and decisions, with the why), reference (where things are).
- past sessions: sessions(text, k) returns the matching facts (decision with why, user
  correction, outcome, open thread) with date and session; session(id) for a whole summary;
  what was literally said: recall(text, k, "turn").
- Before asking the user something they may have told you already, search the memory.

Answers are compact JSON.
