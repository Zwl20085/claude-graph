# Node brief template

Extracted from [ARCHITECTURE.md](ARCHITECTURE.md) §3. This is the exact text an agent receives for one node.

`render_brief(graph, node, run)` produces `brief.md` (saved) and the run prompt
(`prompt.md` = brief + run-time header). Exact shape:

````
# Node: <title>   (graph: <name>, node id: <id>)

You are one node in a graph of engineering tasks run by Claude Graph.
Graph folder: <abs path>
Your deliverables folder: <abs path>/nodes/<id>/deliverables/   (write ALL outputs here)
Working directory: <abs path or custom>

## Goal
<goal>

## Acceptance criterion
<acceptance>

## Inputs from upstream nodes
### <upstream title> (id: <upstream-id>) — status: succeeded
Deliverables:
- <abs path>/nodes/<upstream-id>/deliverables/<path> — <description>
Result summary (first 4000 chars of its result.md):
```text
<...>
```
(repeat per direct upstream node; if none: "None. This is a root node.")

## Scope
In: <scope_in>
Out: <scope_out>

## Already known / ruled out
<already_known>

## Deliverables (required)
- <path> — <description>
(each path relative to the deliverables folder above)

## Do not
- <do_nots...>

## Extra context
<extra_context>

## How to finish
Write every deliverable listed above. Then end your final message with exactly:

## Deliverables
- <path> — one line on what it contains
## Status: DONE
(or `## Status: BLOCKED` followed by one paragraph on why.)
````

The backend parses the final assistant text for `## Status: DONE|BLOCKED`, and
checks that each required deliverable file exists. Missing files or BLOCKED mark
the node `failed` (with reason) even if the CLI exit code is 0.
