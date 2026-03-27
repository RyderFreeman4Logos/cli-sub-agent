#!/usr/bin/env bash
# Pre-push: verify that a csa review has been run on the current HEAD.
# Delegates to the existing review-check script.
exec "$(dirname "$0")/pre-push" "$@"
