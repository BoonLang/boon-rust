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
- Commands that open GUI windows must be launched through
  `cosmic-background-launch --workspace boon-rust -- <command> [args...]` so
  COSMIC receives a background-launch activation token and can place them away
  from the user's active workspace without stealing focus. This includes native
  playgrounds, app_window verification helpers, browsers, and WebExtension
  harnesses.
- Apply `cosmic-background-launch` at the actual window-creating process, not
  only around a parent command that later spawns app_window or browser helpers.
- If a GUI command still opens in the active workspace or steals focus, treat it
  as a windowing/activation bug to fix before continuing routine visible-window
  testing.
