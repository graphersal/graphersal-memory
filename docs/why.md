# Why graphersal-memory

> Pre-release: this page explains the reasoning behind a product that is being built. See the
> [status table](../README.md#status).

## What goes wrong with agent memory today

AI coding agents start every session with an empty head. The usual fix is a memory file: an index
such as `MEMORY.md` plus a few files with rules and notes, loaded into the context at the start of
every session or on every turn. That works for a dozen entries, and it degrades in three ways as
the memory grows:

1. **Cost grows with every entry, on every turn.** The index is part of the context of each turn.
   Twice the memory means twice the tokens for that part of the context, whether or not the turn
   needs it.
2. **Relevant entries get lost.** A rule written three months ago competes with everything written
   since. The agent has to notice it, open the right file, and read it.
3. **History is not recorded at all.** File memory holds what someone decided to write down. What
   happened in the sessions themselves (which bug was found, what was tried and rejected, why a
   decision was made) is gone when the session ends, unless someone copies it by hand.

## What a graph changes

- **Search instead of loading.** The agent asks for the few entries that answer the question
  (`memories("test approval rule")`, `sessions("why was macOS dropped from CI")`), so the cost of
  a lookup stays roughly the same as the memory grows. Only a small, bounded index of the
  most important entries (pinned, recent, most used) stays in the context.
- **Connections.** A memory knows its project, the decision it came from, the session it was
  written in and what it is about. A question about a decision can follow those links to the
  session where it was made.
- **Sessions are first-class.** Every finished session is indexed: raw turns always, and an
  opt-in summary with decisions (with their why), corrections, outcomes and open threads.
- **Rules still reach the harness natively.** The graph is the source, and the rules are exported
  into the files your harness already reads, so nothing depends on the agent remembering to look
  them up.

## Why we built it

We have used a hand-built prototype of this memory on Graphersal's own development since November
2026, and it is where the measured numbers in the README come from. It needed launchd jobs,
scripts, hooks and hand-written configuration. graphersal-memory packages it so that it takes one
command to install and one command to remove.

## Why we ship an A/B with it

Our own measurement is promising but small: one developer, 21 questions, one model. Your memory,
your questions and your harness are different. Instead of asking you to trust our numbers,
graphersal-memory will run the same kind of comparison on your own data, without touching your
live setup: [ab-testing.md](ab-testing.md).

## Hypothesis we still have to verify

We expect the gap to grow with the memory. File memory pays for every entry on every turn and
holds no history. The graph's cost per question stays roughly constant, as long as the exported
index stays bounded and search keeps finding the right entry among more near-misses. This has not
been measured yet. A rerun as the memory grows and a synthetic scale test are planned.
