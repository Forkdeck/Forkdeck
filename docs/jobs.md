[Documentation](README.md) · [Front page](../README.md)

# How a job runs

1. **Fork layer.** Three typed questions are answered before anything starts:
   which agent, how much effort, and whether the change needs a review. An
   answer with confidence under `router.threshold` is "split" and takes the
   cautious branch: the default agent, one effort tier up, and a review.
2. **Work.** The chosen CLI runs headless in the working directory.
3. **Checks.** Your own commands (`checks = ["cargo test"]`) run next. A
   failure goes back to the worker with the output.
4. **Review.** An agent running another vendor's model reads the diff, read-only, and ends with
   `VERDICT: PASS` or `VERDICT: FAIL`. It sees the task and the tree, not the
   worker's summary. A fail goes back to the worker with the review, and the
   worker keeps addressing it until the review passes. `review.max_fix_rounds`
   (5) only caps runaway loops, and fixing also stops when a fix changes
   nothing. A reviewer that crashes or gives no verdict is asked once more. Only the last line of the review counts as
   the verdict, so one quoted from a description or a code comment is ignored,
   and no verdict is never treated as a pass.

Work that starts from an issue is always reviewed, whatever the fork layer
answers, unless `review.mode = "never"`.

Output tokens cost more than input, so the worker is asked for a short report
(what changed, what it could not verify) and the reviewer for one or two lines
per problem. The worker's report becomes the pull request description.

Effort tiers map to models and reasoning effort per CLI in the config, so
"light" can be a small model for a typo and "heavy" the largest one.
