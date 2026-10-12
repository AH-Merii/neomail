#!/bin/sh
# Installs neomail, the terminal email client, from its GitHub releases.
#
#   curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/AH-Merii/neomail/main/install.sh | sh
#   sh install.sh [--version vX.Y.Z] [--to DIR] [--force] [--uninstall]
#
# Everything runs from main(), called on the last line, so a truncated download does nothing.
# Environment:
#   NEOMAIL_INSTALL_DIR       where to put neomail (default: $XDG_BIN_HOME, else ~/.local/bin)
#   NEOMAIL_INSTALL_BASE_URL  for tests and mirrors: the URL of the directory that holds the
#                             tarballs and SHA256SUMS, used instead of the GitHub release
#                             (https, or http on 127.0.0.1 or localhost only)

REPO="AH-Merii/neomail"

usage() {
	cat <<EOF
Installs neomail from the edge pre-release (or the tag given with --version).

Usage: install.sh [options]
  --version TAG   install the release TAG (vX.Y.Z) instead of edge
  --to DIR        install into DIR (default: \$NEOMAIL_INSTALL_DIR, \$XDG_BIN_HOME or ~/.local/bin)
  --force         replace a 'neomail' in DIR that is not this neomail
  --uninstall     remove neomail from DIR
  -h, --help      show this help
EOF
}

say() { printf '%s\n' "$*" >&2; }
die() { say "install.sh: error: $*"; exit 1; }

# The release target for this machine, or nothing if there is no build for it.
detect_target() {
	case "$(uname -s)/$(uname -m)" in
		Linux/x86_64 | Linux/amd64) echo x86_64-unknown-linux-musl ;;
		Darwin/arm64 | Darwin/aarch64) echo aarch64-apple-darwin ;;
		# An x86_64 shell under Rosetta on Apple silicon still gets the native build.
		Darwin/x86_64)
			if [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ]; then
				echo aarch64-apple-darwin
			fi
			;;
	esac
}

# download URL FILE: fails on an HTTP error such as a 404.
download() {
	proto='=https'
	case "$1" in
		http://127.0.0.1[:/]* | http://localhost[:/]*) proto='=http,https' ;;
	esac
	if command -v curl >/dev/null 2>&1; then
		curl --proto "$proto" --proto-redir "$proto" --tlsv1.2 -fsSL -o "$2" "$1"
	elif command -v wget >/dev/null 2>&1; then
		# BusyBox wget (Alpine) has no --https-only; the URLs it gets are https already.
		if [ "$proto" = '=https' ] && wget --help 2>&1 | grep -q -- '--https-only'; then
			wget --https-only -q -O "$2" "$1"
		else
			wget -q -O "$2" "$1"
		fi
	else
		die "neither curl nor wget is installed"
	fi
}

sha256_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d ' ' -f 1
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 "$1" | cut -d ' ' -f 1
	else
		die "neither sha256sum nor shasum is installed, so the download cannot be checked"
	fi
}

# fetch_verified: downloads $asset and SHA256SUMS into $tmp and checks the tarball's checksum.
fetch_verified() {
	rm -f "$tmp/$asset" "$tmp/SHA256SUMS"
	download "$base/$asset" "$tmp/$asset" || { say "cannot download $base/$asset"; return 1; }
	download "$base/SHA256SUMS" "$tmp/SHA256SUMS" || { say "cannot download $base/SHA256SUMS"; return 1; }
	want=$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1; exit }' "$tmp/SHA256SUMS")
	[ -n "$want" ] || { say "SHA256SUMS has no line for $asset"; return 1; }
	got=$(sha256_of "$tmp/$asset")
	[ "$got" = "$want" ] || { say "checksum mismatch for $asset: expected $want, got $got"; return 1; }
}

# is_ours FILE: true if FILE is a neomail binary (d99kris/nmail and others print something else).
is_ours() {
	case "$("$1" --version </dev/null 2>/dev/null)" in
		"neomail "*) return 0 ;;
		*) return 1 ;;
	esac
}

cleanup() {
	[ -n "${tmp:-}" ] && rm -rf "$tmp"
	[ -n "${partial:-}" ] && rm -f "$partial"
}

uninstall() {
	[ -e "$dest" ] || die "there is no neomail in $dir"
	is_ours "$dest" || die "$dest is not neomail; leaving it"
	rm -f "$dest" || die "cannot remove $dest"
	say "Removed $dest."
	say ""
	say "Your config, cache and keyring entries are still there. To remove them too:"
	say "  rm -r \"${XDG_CONFIG_HOME:-$HOME/.config}/neomail\" \"${XDG_DATA_HOME:-$HOME/.local/share}/neomail\""
	say "The keyring entries (services 'neomail' and 'neomail-oauth') need neomail itself: reinstall,"
	say "run one of these for each account, then uninstall again:"
	say "  neomail account delete-password NAME   # an account with a password"
	say "  neomail account logout NAME            # a Gmail account (also revokes the sign-in)"
}

main() {
	set -u
	tag=edge
	dir="${NEOMAIL_INSTALL_DIR:-${XDG_BIN_HOME:-$HOME/.local/bin}}"
	force=0
	mode=install
	while [ $# -gt 0 ]; do
		case "$1" in
			--version) [ $# -ge 2 ] || die "--version needs a tag"; tag=$2; shift ;;
			--to) [ $# -ge 2 ] || die "--to needs a directory"; dir=$2; shift ;;
			--force) force=1 ;;
			--uninstall) mode=uninstall ;;
			-h | --help) usage; exit 0 ;;
			*) usage >&2; die "unknown option: $1" ;;
		esac
		shift
	done
	case "$tag" in
		edge | v[0-9]*) ;;
		*) die "--version takes a tag such as v0.1.0, not '$tag'" ;;
	esac
	case "$dir" in
		*?/) dir=${dir%/} ;;
	esac
	dest="$dir/neomail"
	if [ "$mode" = uninstall ]; then
		uninstall
		return
	fi

	command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 ||
		die "neither sha256sum nor shasum is installed, so the download cannot be checked"
	target=$(detect_target)
	if [ -z "$target" ]; then
		say "There is no prebuilt neomail for $(uname -s) $(uname -m) yet. Build it from source"
		say "(Rust 1.88 or newer):"
		say "  cargo install --locked --git https://github.com/$REPO neomail"
		exit 1
	fi
	if [ -d "$dest" ]; then
		die "$dest is a directory"
	fi
	if [ -e "$dest" ] && [ "$force" = 0 ] && ! is_ours "$dest"; then
		die "$dest exists and is not neomail (d99kris/nmail and neomail are different programs); pick another directory with --to, or replace it with --force"
	fi
	for tool in tar mktemp; do
		command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
	done

	asset="neomail-$target.tar.gz"
	base="${NEOMAIL_INSTALL_BASE_URL:-https://github.com/$REPO/releases/download/$tag}"
	base="${base%/}"
	tmp=$(mktemp -d) || die "cannot create a temporary directory"
	partial=""
	trap cleanup EXIT
	# A signal skips the EXIT trap unless it is caught; exit runs the cleanup.
	trap 'exit 129' HUP
	trap 'exit 130' INT
	trap 'exit 141' PIPE
	trap 'exit 143' TERM

	say "Downloading $asset ($tag) from $base"
	if ! fetch_verified; then
		if [ "$tag" = edge ]; then
			say "edge may be in the middle of a republish; trying again in 10 seconds."
		else
			say "Trying again in 10 seconds."
		fi
		sleep 10
		fetch_verified || die "the download failed twice; nothing was installed"
	fi
	tar -xzf "$tmp/$asset" -C "$tmp" || die "cannot unpack $asset"
	bin="$tmp/neomail-$target/neomail"
	[ -f "$bin" ] || die "$asset does not hold neomail-$target/neomail"

	mkdir -p "$dir" || die "cannot create $dir"
	partial="$dir/.neomail.partial.$$"
	cp "$bin" "$partial" || die "cannot write to $dir"
	chmod 755 "$partial"
	# Checked here, not in $tmp: /tmp is often mounted noexec. The EXIT trap removes $partial.
	is_ours "$partial" || die "the downloaded neomail does not run on this machine"
	mv -f "$partial" "$dest" || die "cannot move neomail into $dir"
	partial=""
	say "Installed $("$dest" --version) to $dest"

	case ":$PATH:" in
		*":$dir:"*) ;;
		*)
			say ""
			say "$dir is not on your PATH. Add it in your shell's startup file:"
			say "  sh, bash, zsh (~/.profile, ~/.bashrc, ~/.zshrc):  export PATH=\"$dir:\$PATH\""
			say "  fish:                                              fish_add_path \"$dir\""
			;;
	esac
	found=$(command -v neomail 2>/dev/null || true)
	if [ -n "$found" ] && [ "$found" != "$dest" ]; then
		say ""
		say "Warning: 'neomail' on your PATH runs $found, not $dest."
		say "Remove it, or put $dir before ${found%/*} in PATH."
	fi
	say ""
	say "Next: write a config and run 'neomail sync', as in \"Try it\" in the README:"
	say "  https://github.com/$REPO#try-it"
}

main "$@"
