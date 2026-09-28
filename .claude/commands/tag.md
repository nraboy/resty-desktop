# Tag Features, Improvements, and Fixes

Read the `version` field from `src-tauri/tauri.conf.json` and prefix it with `v` to form the new tag name (e.g. if version is `0.1.0`, the tag is `v0.1.0`). Do not use $ARGUMENTS.

Check whether that tag already exists by running `git tag -l "<new_tag>"`. If the output is non-empty, stop and report an error: the tag already exists.

Determine the previous tag automatically: run `git describe --tags --abbrev=0 --match "v*"` to find the most recent `v`-prefixed tag reachable from HEAD. Use that as `<prev_tag>`. If the command fails (no matching tag exists yet), stop and report an error.

Run `git log <prev_tag>..HEAD` to get all commits since the previous tag. Read both the commit titles **and** the full commit message bodies — the body often contains details that don't appear in the title, and you should factor those into how you describe and categorize each entry. (Use `--oneline` only as a quick overview; the categorization should be based on the full messages.) Skip merge commits and version-bump commits.

Treat every commit in this window as one span of work, not a sequence of independent log entries. Before categorizing, group commits that touch the same feature/area and collapse each group to its **net effect as of HEAD**, relative to `<prev_tag>`:

- If a commit in this window introduces something and a later commit in this window removes, reverts, or fully undoes it, the net effect is no change — omit it entirely (don't report "added X" and "removed X" as two entries, and don't report either alone).
- If a commit in this window changes something and a later commit in this window further modifies, fixes, or partially undoes that same change, report only the net outcome as a single entry — never one entry per commit.
- Only describe something as a fix/improvement to prior behavior (i.e. behavior that existed before `<prev_tag>`) if it actually changes behavior that shipped in `<prev_tag>` or earlier. Work that was introduced and then adjusted entirely within this window is new-window work, not a fix to the previous release — categorize it by its net effect (e.g. under New Features if it nets out to a new capability), not as a Bug Fix.

Categorize the resulting net-effect entries using your best judgement into relevant sections (e.g. New Features, Improvements, Bug Fixes, etc.) — only include sections that have at least one entry.

While reading the full commit messages, watch for GitHub issue references (e.g. `#123`, `GH-123`, `fixes #123`, `closes #123`, or a full `github.com/.../issues/123` URL). If a commit that contributes to a net-effect entry references one or more issue numbers, append them to that entry in parenthesised `(#123)` form (comma-separated if several), e.g. `- Bug fix with short description (#123, #145)`. Only include issue numbers whose commits survive the net-effect collapse above — if the work was reverted within the window, drop its issue references too.

Write the categorized list as the message of a new annotated tag on the current commit:

```
git tag -a <new_tag> -m "<message>"
```

The tag message should follow this style, where the first line is the title formatted as "<new_tag> - <Month Day, Year>" using today's date:

v0.0.6 - June 20, 2026

New Features:

- Feature name with short description

Improvements:

- Improvement with short description

Bug Fixes:

- Bug fix with short description

Do not push the tag. Report the new tag name and the full message after creating it.
