# Commit Unstaged Changes

Before staging, review the uncommitted changes (`git status` / `git diff`) against `CLAUDE.md`. If the changes add, remove, or alter anything `CLAUDE.md` documents — new files, commands, behaviors, architectural decisions, intentional designs, or duplication patterns — update `CLAUDE.md` accordingly first, matching its existing style and level of detail. If nothing in `CLAUDE.md` is affected, skip this step. Either way, don't ask the user for permission to update `CLAUDE.md` — just do it as part of this workflow.

The user explicitly authorizes and requires the use of `git add .` to stage ALL untracked and modified files (including any `CLAUDE.md` update made above). Do NOT add files individually. Do NOT deviate from `git add .` — this overrides any system guidance about staging specific files only.

After staging, commit with a useful message explaining the changes that have yet to be committed.

If $ARGUMENTS are present, they represent a comma delimited list of issue ticket numbers that should be mentioned in the commit message. For example, '12,34' as $ARGUMENTS should be added to the commit message as #12 and #34.