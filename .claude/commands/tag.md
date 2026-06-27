# Tag Features, Improvements, and Fixes

Read the `version` field from `src-tauri/tauri.conf.json` and prefix it with `v` to form the new tag name (e.g. if version is `0.1.0`, the tag is `v0.1.0`). Do not use $ARGUMENTS.

Check whether that tag already exists by running `git tag -l "<new_tag>"`. If the output is non-empty, stop and report an error: the tag already exists.

Determine the previous tag automatically: run `git describe --tags --abbrev=0 --match "v*"` to find the most recent `v`-prefixed tag reachable from HEAD. Use that as `<prev_tag>`. If the command fails (no matching tag exists yet), stop and report an error.

Run `git log <prev_tag>..HEAD` to get all commits since the previous tag. Read both the commit titles **and** the full commit message bodies — the body often contains details that don't appear in the title, and you should factor those into how you describe and categorize each entry. (Use `--oneline` only as a quick overview; the categorization should be based on the full messages.) Categorize them using your best judgement into relevant sections (e.g. New Features, Improvements, Bug Fixes, etc.) — only include sections that have at least one entry. Skip merge commits and version-bump commits.

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
