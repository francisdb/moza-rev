#!/usr/bin/env bash
# Install, run and remove a generated package in a clean disposable container.
set -euo pipefail

if [[ $# != 3 ]]; then
    echo "Usage: $0 IMAGE PACKAGE_BUILD_DIRECTORY amd64|arm64" >&2
    exit 1
fi
image=$1
packages=$(realpath "$2")
arch=$3
case "$arch" in amd64|arm64) ;; *) exit 1 ;; esac
case "$image" in
    ubuntu:*|debian:*) format=deb ;;
    fedora:*) format=rpm ;;
    archlinux/archlinux:*) format=archlinux ;;
    *) echo "Unsupported test image: $image" >&2; exit 1 ;;
esac

docker run --rm -i --platform "linux/$arch" \
    --volume "$packages:/packages:ro" "$image" bash -se -- "$format" "$arch" <<'CONTAINER'
format=$1
arch=$2
set -- /packages/packages/linux-"$arch"/"$format"/*
test "$#" -eq 1
test -f "$1"
case "$format" in
    deb)
        apt-get update -qq
        DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$1"
        ;;
    rpm) dnf install -y "$1" ;;
    archlinux)
        pacman -Syu --noconfirm
        pacman -U --noconfirm "$1"
        ;;
esac

moza-rev --version
moza-rev --help >/dev/null
test -r /usr/share/licenses/moza-rev/LICENSE
# Minimal images may exclude /usr/share/doc through package-manager policy.
grep -Fx 'ExecStart=/usr/bin/moza-rev' /usr/lib/systemd/user/moza-rev.service
if grep -Eq '^User=|^Group=|@USER@' /usr/lib/systemd/user/moza-rev.service; then
    echo 'The user service must not contain system-service identity settings.' >&2
    exit 1
fi

case "$format" in
    deb) apt-get remove -y moza-rev ;;
    rpm) dnf remove -y moza-rev ;;
    archlinux) pacman -R --noconfirm moza-rev ;;
esac
test ! -e /usr/bin/moza-rev
test ! -e /usr/lib/systemd/user/moza-rev.service
CONTAINER
