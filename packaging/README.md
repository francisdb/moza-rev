# Building native packages

[`native-packages.yaml`](../native-packages.yaml) defines installed files,
architectures, dependency mappings, and package formats. Packaging tools are
build-time dependencies; users installing a package do not need Rust, Ruby,
Go, or native-packages.

| Target | Formats |
| --- | --- |
| Linux x86-64 (`linux-amd64`) | DEB, RPM, Arch Linux |
| Linux ARM64 (`linux-arm64`) | DEB, RPM |

moza-rev currently discovers serial devices and Steam games through Linux
paths. Although native-packages also supports macOS and Windows installers,
those targets require application porting and hardware validation first.
Arch Linux ARM packages are not advertised as official Arch Linux support.

## Local build

Install Rust, Ruby 3.2 or newer, Go, and the native build dependencies.
On Debian/Ubuntu, the additional system tools are:

```sh
sudo apt install build-essential pkg-config libudev-dev libarchive-tools binutils xz-utils
gem install native-packages --version 0.8.1 --no-document
go install github.com/goreleaser/nfpm/v2/cmd/nfpm@v2.47.0
export PATH="$(go env GOPATH)/bin:$PATH"
```

On an x86-64 Linux host, from the repository root:

```sh
bash packaging/build.sh amd64
native-packages validate
native-packages doctor --target linux-amd64
native-packages build --version 0.1.0 --target linux-amd64
```

Use the version in `Cargo.toml`. For ARM64, run on an ARM64 Linux host and
replace `amd64` with `arm64` in those commands. Cross-compiling also needs the
Rust target, linker, and libudev sysroot for the destination architecture;
native-packages packages the supplied executable rather than compiling it.

Output goes to `dist/packages/VERSION/`, including package files, checksums,
and a build manifest. The output directory must not already exist; use
`--output dist/another-build` for a subsequent build.

DEB/RPM dependencies are derived from the executable's ELF libraries and
glibc symbol versions. The configuration maps `libudev.so.1` to `libudev1`
on Debian and `systemd-libs` on Fedora. Arch dependencies are explicit.
Build on the oldest supported distribution: compiling on a newer host can
raise the glibc requirement recorded in the package.

## Installation tests

Docker tests install the package and its dependencies, run `--version` and
`--help`, check the user service and license, and remove the package:

```sh
bash packaging/test-install.sh ubuntu:24.04 dist/packages/0.1.0 amd64
bash packaging/test-install.sh debian:13-slim dist/packages/0.1.0 amd64
bash packaging/test-install.sh fedora:latest dist/packages/0.1.0 amd64
bash packaging/test-install.sh archlinux/archlinux:latest dist/packages/0.1.0 amd64
```

Run ARM64 tests on an ARM64 host or with Docker's ARM64 emulation configured.
These tests require no wheel and do not start the daemon. The package's user
service runs as the logged-in user with existing device permissions; it does
not use the source installer's `User=@USER@` system-service template.

## GitHub Actions and releases

The `Native packages` workflow builds and tests each architecture on a native
Ubuntu 24.04 runner. Changes to code or packaging trigger builds on pushes and
pull requests; `workflow_dispatch` can also build the selected ref. The two
`packages-linux-*` artifacts contain complete native-packages build outputs.

To publish, create a stable GitHub release in `francisdb/moza-rev` with a tag
matching `Cargo.toml` (for example, `v0.1.0`). The release event builds and tests
both architectures, combines their verified build manifests, and attaches the
five packages and a shared checksum file to that existing release. It does
not create tags, write release notes, or publish to distribution repositories.
Prereleases are not packaged by this workflow.

Only the publish job has `contents: write`; pull request builds need no secrets.
Third-party Actions are pinned to commits, and native-packages/nFPM versions
are pinned in both the configuration and workflow. Keep those pins in sync
when upgrading.
