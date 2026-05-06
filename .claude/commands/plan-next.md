Read CLAUDE.md for context.
Run `gh issue list --state open` and `gh issue list --state open --milestone` to see all open issues and milestones.

For each issue with the `blocked` label, parse its body for cross-repo prereq links of the form `<owner>/<repo>#<N>` (e.g. `JDonaghy/vimcode#296`). For each prereq, run `gh issue view <N> --repo <owner>/<repo> --json state -q .state` to check whether it's `CLOSED` or `OPEN`. An issue is **ready to unblock** only when ALL of its linked prereqs are `CLOSED`.

Do not scan or explore the src/ directory yet.

Group findings into three buckets and report:

1. **Ready to work on** — open, not labelled `blocked`. The normal backlog.
2. **Newly unblocked** — was labelled `blocked` but all linked prereqs are now `CLOSED`. Highest leverage (someone was waiting; now they can proceed). Recommend removing the `blocked` label as part of the pickup.
3. **Still blocked** — labelled `blocked` with at least one `OPEN` prereq. List the prereq numbers + titles so the user knows what would unblock it; do not recommend these.

Within bucket 1, highlight priority + intra-repo dependencies, and recommend what to work on next.
List what files you expect to touch and why.
Then wait for my confirmation before doing anything.
