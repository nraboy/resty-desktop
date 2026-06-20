# Tag Features, Improvements, and Fixes

Parse $ARGUMENTS as a comma-delimited string. For example "v0.0.1,v0.0.2" means the previous tag is "v0.0.1" and the new tag is "v0.0.2". If $ARGUMENTS does not contain exactly two comma-separated values, stop and report an error.

Verify that the previous tag exists in the repo (`git tag -l <prev_tag>`). If it does not exist, stop and report an error.

Run `git log <prev_tag>..HEAD --oneline` to get all commits since the previous tag. Categorize them using your best judgement into relevant sections (e.g. New Features, Improvements, Bug Fixes, etc.) — only include sections that have at least one entry. Skip merge commits and version-bump commits.

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