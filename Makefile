# Development shortcuts. The real build is Meson + Cargo (see README.md).

BUILDDIR ?= _build
RPMDIR   := $(CURDIR)/rpmbuild
MOCK_CHROOT ?= fedora-45-x86_64
VERSION  := $(shell sed -n "s/^  version: '\(.*\)',$$/\1/p" meson.build)
NAME     := gatefold

.PHONY: all build run test lint fmt tarballs srpm rpm mock clean

all: build

$(BUILDDIR)/build.ninja:
	meson setup $(BUILDDIR) -Dprofile=development

build: $(BUILDDIR)/build.ninja
	meson compile -C $(BUILDDIR)

# Runs the development build uninstalled, with its schema from the build dir.
run: build
	GSETTINGS_SCHEMA_DIR=$(BUILDDIR)/data $(BUILDDIR)/app/$(NAME)

test: build
	meson test -C $(BUILDDIR) --print-errorlogs

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

fmt:
	cargo fmt --all

Cargo.lock:
	cargo generate-lockfile

# Source tarball (working tree) + vendored Rust dependencies, for rpmbuild.
tarballs: Cargo.lock
	mkdir -p $(RPMDIR)/SOURCES
	tar --exclude-vcs --exclude=./$(BUILDDIR) --exclude=./target --exclude=./vendor \
	    --exclude=./rpmbuild --exclude=./.cargo \
	    --transform 's,^\.,$(NAME)-$(VERSION),' \
	    -cJf $(RPMDIR)/SOURCES/$(NAME)-$(VERSION).tar.xz .
	rm -rf vendor
	cargo vendor --locked --versioned-dirs vendor > /dev/null
	tar -cJf $(RPMDIR)/SOURCES/$(NAME)-$(VERSION)-vendor.tar.xz vendor
	rm -rf vendor

srpm: tarballs
	rpmbuild -bs --define '_topdir $(RPMDIR)' $(NAME).spec

# Quick build on this machine (needs the BuildRequires installed).
rpm: tarballs
	rpmbuild -ba --define '_topdir $(RPMDIR)' $(NAME).spec

# Clean build in a fresh Fedora 45 chroot; catches missing BuildRequires.
mock: srpm
	mock -r $(MOCK_CHROOT) --resultdir $(RPMDIR)/mock \
	    --rebuild $$(ls -t $(RPMDIR)/SRPMS/$(NAME)-$(VERSION)-*.src.rpm | head -n1)

clean:
	rm -rf $(BUILDDIR) target vendor $(RPMDIR)
