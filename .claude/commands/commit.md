# Commit Unstaged Changes

Stage everything with the 'git add .' command and commit with a useful message explaining the changes that have yet to be committed.

If $ARGUMENTS are present, they represent a comma delimited list of issue ticket numbers that should be mentioned in the commit message. For example, '12,34' as $ARGUMENTS should be added to the commit message as #12 and #34.

It is very important that all files are added, not just the files that were changed in the current context. It is also very important that the comma delimited list of tickets be treated as tickets if present.