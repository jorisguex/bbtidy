#!/bin/sh
set -eu

script_directory=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
repository_directory=$(dirname -- "$script_directory")
layer_directory="$repository_directory/tests/fixtures/corpus/expected"
build_directory=${BBTIDY_BITBAKE_BUILD_DIR:-${BUILDDIR:-}}
bitbake_command=${BBTIDY_BITBAKE:-bitbake}
bitbake_target=${BBTIDY_BITBAKE_TARGET:-example}

if [ -z "$build_directory" ]; then
    echo "Set BBTIDY_BITBAKE_BUILD_DIR or source a build environment that exports BUILDDIR." >&2
    exit 2
fi

if ! command -v "$bitbake_command" >/dev/null 2>&1; then
    echo "BitBake command not found: $bitbake_command" >&2
    exit 2
fi

if [ ! -d "$build_directory/conf" ]; then
    echo "Configured BitBake build directory not found: $build_directory" >&2
    exit 2
fi

cd "$build_directory"
if ! "$bitbake_command" --parse-only "$bitbake_target"; then
    echo "BitBake did not parse the corpus target '$bitbake_target'." >&2
    echo "Use a disposable build and enable the formatted corpus layer:" >&2
    echo "  bitbake-layers add-layer $layer_directory" >&2
    exit 1
fi
