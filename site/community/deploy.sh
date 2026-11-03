#!/bin/sh
# Compatibility entry point; the project and community sites deploy together.
set -eu
exec sh "$(dirname "$0")/../deploy.sh"
