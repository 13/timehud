#!/bin/bash
# Default to XWayland/X11 for reliable always-on-top behavior.
if [ -z "${QT_QPA_PLATFORM:-}" ]; then
  export QT_QPA_PLATFORM=xcb
fi
exec {{ python-executable }} -m timehud.main "$@"
