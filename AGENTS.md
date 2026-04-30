## Repository Rules

- Do not perform Git write operations without explicit user permission. This
  includes `git add`, `git commit`, `git reset`, `git checkout`, `git worktree`,
  `git branch`, `git merge`, `git rebase`, `git tag`, and `git push`.
- Git read operations such as `git status`, `git diff`, `git log`, and
  `git show` are allowed when needed for verification or context.
- Do not create extra clean-checkout or clean-worktree verification copies unless
  the user explicitly asks for one. Use the current development checkout for
  verification by default.
- Keep `target/` and `.boon-local/` local to this machine; do not duplicate them
  into temporary checkouts for routine verification.
