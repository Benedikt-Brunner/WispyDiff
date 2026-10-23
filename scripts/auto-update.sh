#!/usr/bin/env bash
# Keeps the local installation on the latest `main`. A launchd agent (macOS) or systemd user timer
# (Linux) runs `run` every 10 minutes; when `main` moved, it builds it in a private clone under the
# updater dir (never your checkout) and swaps the result in: /Applications/WispyDiff.app on macOS,
# ~/.local/bin/wispydiff on Linux. A running app keeps going; a notification says to restart it.
#
#   auto-update.sh enable          install and start the background job (it updates right away)
#   auto-update.sh disable         stop and remove it (the build cache stays in the updater dir)
#   auto-update.sh run [--force]   check once; --force rebuilds even when up to date
#   auto-update.sh status          installed commit, latest main, recent log
#
# The job runs a copy of this script from the updater dir, refreshed from each build of `main`.
# Overrides (testing; `enable` keeps them for the job): WISPY_UPDATER_REPO, WISPY_UPDATER_BRANCH,
# WISPY_UPDATER_DIR, WISPY_INSTALL_DIR.
set -euo pipefail

REPO=${WISPY_UPDATER_REPO:-https://github.com/Benedikt-Brunner/WispyDiff.git}
BRANCH=${WISPY_UPDATER_BRANCH:-main}
LABEL=dev.wispydiff.updater
INTERVAL_MIN=10
# A failed build of the same commit is retried after this long (it may have been the network).
RETRY_FAILED_MIN=120

case "$(uname -s)" in
  Darwin)
    DIR=${WISPY_UPDATER_DIR:-$HOME/Library/Application Support/WispyDiff-updater}
    INSTALL_DIR=${WISPY_INSTALL_DIR:-/Applications}
    TARGET=$INSTALL_DIR/WispyDiff.app
    ;;
  Linux)
    DIR=${WISPY_UPDATER_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/wispydiff-updater}
    INSTALL_DIR=${WISPY_INSTALL_DIR:-$HOME/.local/bin}
    TARGET=$INSTALL_DIR/wispydiff
    ;;
  *) echo "auto-update: unsupported OS $(uname -s)" >&2; exit 1 ;;
esac
SRC=$DIR/src
LOG=$DIR/updater.log

log() {
  local line
  line="$(date '+%F %T') $*"
  echo "$line" >> "$LOG"
  if [ -t 1 ]; then echo "$line"; fi
}

notify() {
  if [ "$(uname -s)" = Darwin ]; then
    osascript -e "display notification \"$1\" with title \"WispyDiff\"" > /dev/null 2>&1 || true
  elif command -v notify-send > /dev/null; then
    notify-send WispyDiff "$1" || true
  fi
}

# One run at a time: a manual run can overlap the timer's.
lock() {
  if ! mkdir "$DIR/lock" 2> /dev/null; then
    local pid
    pid=$(cat "$DIR/lock/pid" 2> /dev/null || true)
    if [ -n "$pid" ] && kill -0 "$pid" 2> /dev/null; then
      log "another update is running (pid $pid)"
      exit 0
    fi
    rm -rf "$DIR/lock"
    mkdir "$DIR/lock"
  fi
  echo $$ > "$DIR/lock/pid"
  trap 'rm -rf "$DIR/lock"' EXIT
}

build() {
  export CI=true # pnpm: never prompt
  pnpm install --frozen-lockfile
  if [ "$(uname -s)" = Darwin ]; then
    pnpm tauri build
  else
    pnpm tauri build --no-bundle
  fi
}

# Swaps the new build in with renames, so the target is never half-written.
install_build() {
  mkdir -p "$INSTALL_DIR"
  local staged=$INSTALL_DIR/.$(basename "$TARGET").new old=$INSTALL_DIR/.$(basename "$TARGET").old
  rm -rf "$staged" "$old"
  if [ "$(uname -s)" = Darwin ]; then
    ditto "$SRC/target/release/bundle/macos/WispyDiff.app" "$staged"
    if [ -e "$TARGET" ]; then mv "$TARGET" "$old"; fi
    mv "$staged" "$TARGET"
    rm -rf "$old"
  else
    cp "$SRC/target/release/wispydiff" "$staged"
    mv -f "$staged" "$TARGET"
  fi
}

# The job runs the copy in the updater dir, so it picks up changes to this script with `main`.
copy_script() {
  cp "$1" "$DIR/auto-update.sh.new"
  chmod +x "$DIR/auto-update.sh.new"
  mv -f "$DIR/auto-update.sh.new" "$DIR/auto-update.sh" # a rename: a running copy reads on unharmed
}

run() {
  local force=${1:-}
  mkdir -p "$DIR"
  lock
  # Keep the log small.
  if [ -f "$LOG" ] && [ "$(wc -c < "$LOG")" -gt 500000 ]; then
    tail -n 1000 "$LOG" > "$LOG.tmp" && mv "$LOG.tmp" "$LOG"
  fi

  if [ ! -d "$SRC/.git" ]; then
    log "cloning $REPO"
    rm -rf "$SRC"
    git clone --quiet --branch "$BRANCH" "$REPO" "$SRC"
  fi
  # Offline is normal on a laptop; just try again next time.
  if ! git -C "$SRC" fetch --quiet "$REPO" "$BRANCH" 2> /dev/null; then
    if [ -t 1 ]; then echo "fetching $REPO failed (offline?)" >&2; fi
    return 0
  fi
  local sha subject
  sha=$(git -C "$SRC" rev-parse FETCH_HEAD)
  subject=$(git -C "$SRC" log -1 --format=%s "$sha")

  if [ "$force" != --force ]; then
    if [ "$sha" = "$(cat "$DIR/installed" 2> /dev/null)" ] && [ -e "$TARGET" ]; then
      if [ -t 1 ]; then echo "up to date: ${sha:0:9} $subject"; fi
      return 0
    fi
    if [ "$sha" = "$(cat "$DIR/failed" 2> /dev/null)" ] \
      && [ -n "$(find "$DIR/failed" -mmin -"$RETRY_FAILED_MIN")" ]; then
      return 0
    fi
  fi

  git -C "$SRC" checkout --quiet --force --detach "$sha"
  git -C "$SRC" clean -fdq # untracked files only; target/ and node_modules/ are ignored, so kept
  log "building ${sha:0:9} $subject"
  local started=$SECONDS
  if ! (cd "$SRC" && build) > "$DIR/build.log" 2>&1; then
    echo "$sha" > "$DIR/failed"
    log "build failed, see $DIR/build.log"
    notify "Building the latest main (${sha:0:9}) failed — see $DIR/build.log"
    return 1
  fi
  install_build
  echo "$sha" > "$DIR/installed"
  rm -f "$DIR/failed"
  if [ -f "$SRC/scripts/auto-update.sh" ]; then copy_script "$SRC/scripts/auto-update.sh"; fi
  log "installed ${sha:0:9} to $TARGET ($(( SECONDS - started ))s)"

  local running=
  if [ "$(uname -s)" = Darwin ]; then
    pgrep -f "$TARGET/Contents/MacOS/" > /dev/null && running=1
  else
    pgrep -x wispydiff > /dev/null && running=1
  fi
  if [ -n "$running" ]; then
    notify "Updated to “$subject” — restart WispyDiff to use it"
  else
    notify "Updated to “$subject”"
  fi
}

# Settings passed to `enable` stay with the job.
job_env() {
  echo "PATH=$PATH" # the job's default PATH has neither cargo nor pnpm
  local var
  for var in WISPY_UPDATER_REPO WISPY_UPDATER_BRANCH WISPY_UPDATER_DIR WISPY_INSTALL_DIR; do
    if [ -n "${!var:-}" ]; then echo "$var=${!var}"; fi
  done
}

xml_escape() {
  sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' <<< "$1"
}

enable() {
  mkdir -p "$DIR"
  copy_script "$0"
  if [ "$(uname -s)" = Darwin ]; then
    local plist=$HOME/Library/LaunchAgents/$LABEL.plist vars="" line
    while IFS= read -r line; do
      vars+="    <key>$(xml_escape "${line%%=*}")</key><string>$(xml_escape "${line#*=}")</string>"$'\n'
    done < <(job_env)
    mkdir -p "$(dirname "$plist")"
    cat > "$plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>$(xml_escape "$DIR/auto-update.sh")</string>
    <string>run</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
$vars  </dict>
  <key>StartInterval</key><integer>$(( INTERVAL_MIN * 60 ))</integer>
  <key>RunAtLoad</key><true/>
  <key>Nice</key><integer>10</integer>
  <key>StandardOutPath</key><string>$(xml_escape "$LOG")</string>
  <key>StandardErrorPath</key><string>$(xml_escape "$LOG")</string>
</dict>
</plist>
EOF
    launchctl bootout "gui/$(id -u)/$LABEL" 2> /dev/null || true
    launchctl bootstrap "gui/$(id -u)" "$plist"
  else
    local units=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user vars="" line
    while IFS= read -r line; do
      vars+="Environment=\"$line\""$'\n'
    done < <(job_env)
    mkdir -p "$units"
    cat > "$units/wispydiff-updater.service" << EOF
[Unit]
Description=Rebuild WispyDiff from the latest $BRANCH

[Service]
Type=oneshot
${vars}Nice=10
ExecStart=/bin/bash "$DIR/auto-update.sh" run
StandardOutput=append:$LOG
StandardError=append:$LOG
EOF
    cat > "$units/wispydiff-updater.timer" << EOF
[Unit]
Description=Check for new WispyDiff commits every $INTERVAL_MIN minutes

[Timer]
OnActiveSec=0
OnUnitActiveSec=${INTERVAL_MIN}min

[Install]
WantedBy=timers.target
EOF
    systemctl --user daemon-reload
    systemctl --user enable --now wispydiff-updater.timer
  fi
  echo "Auto-update enabled: checks $BRANCH every $INTERVAL_MIN minutes and installs to $TARGET."
  echo "The first build runs now and takes a few minutes; follow it with: tail -f \"$DIR/build.log\""
}

disable() {
  if [ "$(uname -s)" = Darwin ]; then
    launchctl bootout "gui/$(id -u)/$LABEL" 2> /dev/null || true
    rm -f "$HOME/Library/LaunchAgents/$LABEL.plist"
  else
    local units=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user
    systemctl --user disable --now wispydiff-updater.timer 2> /dev/null || true
    rm -f "$units/wispydiff-updater.timer" "$units/wispydiff-updater.service"
    systemctl --user daemon-reload
  fi
  echo "Auto-update disabled. The build cache stays in $DIR; delete it to free the space."
}

status() {
  local enabled=no installed latest
  if [ "$(uname -s)" = Darwin ]; then
    launchctl print "gui/$(id -u)/$LABEL" > /dev/null 2>&1 && enabled=yes
  else
    systemctl --user is-enabled --quiet wispydiff-updater.timer 2> /dev/null && enabled=yes
  fi
  installed=$(cat "$DIR/installed" 2> /dev/null || true)
  latest=$(git ls-remote "$REPO" "refs/heads/$BRANCH" 2> /dev/null | cut -c1-40 || true)
  echo "enabled:   $enabled"
  if [ -n "$installed" ] && [ -d "$SRC/.git" ]; then
    echo "installed: ${installed:0:9} $(git -C "$SRC" log -1 --format=%s "$installed" 2> /dev/null)"
  else
    echo "installed: none by the updater"
  fi
  echo "$BRANCH:      ${latest:0:9}${latest:+$([ "$latest" = "$installed" ] && echo " (up to date)")}"
  if [ -f "$DIR/failed" ]; then echo "failed:    $(cut -c1-9 "$DIR/failed") (see $DIR/build.log)"; fi
  if [ -f "$LOG" ]; then
    echo
    tail -n 5 "$LOG"
  fi
}

case "${1:-}" in
  enable) enable ;;
  disable) disable ;;
  run) run "${2:-}" ;;
  status) status ;;
  *) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
