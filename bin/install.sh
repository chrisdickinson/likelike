#!/bin/bash

if ! &>/dev/null command -v watchexec; then
  brew install watchexec
fi

if ! &>/dev/null command -v likelike; then
  gh release download --repo chrisdickinson/likelike -p '*x64_macos*'
  tar zxfv likelike*.tar.gz
  rm likelike*.tar.gz
  mv likelike ~/bin/
fi

log_file=~/.local/state/likelike/log
watchexec_path=$(command -v watchexec)
likelike_path=$(command -v likelike)
notes_dir=~/notes/

if [ ! -e "$(dirname "$log_file")" ]; then
  mkdir -p "$(dirname "$log_file")"
fi

cat > ~/Library/LaunchAgents/us.neversaw.likelike.plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>KeepAlive</key>
  <true/>

	<key>Label</key>
  <string>us.neversaw.likelike</string>

	<key>ProgramArguments</key>
  <array>
    <string>${watchexec_path}</string>
    <string>-e</string>
    <string>md</string>
    <string>-w</string>
    <string>${notes_dir}</string>
    <string>-p</string>
    <string>--</string>
    <string><![CDATA[${likelike_path} import ${notes_dir} && cp "$HOME/Library/Application Support/likelike/db.sqlite3" "$HOME/blog/"]]></string>
  </array>

	<key>RunAtLoad</key>
  <true/>

	<key>StandardErrorPath</key>
  <string>${log_file}</string>

	<key>StandardOutPath</key>
  <string>${log_file}</string>

	<key>WorkingDirectory</key>
  <string>${notes_dir}</string>
</dict>
</plist>
EOF
launchctl load ~/Library/LaunchAgents/us.neversaw.likelike.plist
