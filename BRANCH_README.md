# feat/comment-hunk

This branch depends on the following branches, in this order:

1. `feat/recoverable-config`
2. `feat/markdown-preview`
3. `dev-3-way-diff-second-attempt`

`feat/comment-hunk` is stacked on top of that sequence. Each later branch builds on the one before it.

The checkout is currently rebased onto `dev-3-way-diff-second-attempt` (`83cb6933e`, `fix(diff): keep a deleted file's diff pane pathless`). `feat/markdown-preview` (`769526bcb`) already contains `feat/recoverable-config` (`3bbd6528b`). Those two tips are not ancestors of this branch yet.
