include-path := $(abs_srctree)/src $(abs_srctree)/src/include $(abs_srctree)/fs

out-bins := kernel.exe

kernel.exe-objs =
kernel.exe-hdrs = kernel.link.ld
kernel.exe-libs = boot/objs.a dev/objs.a proc/objs.a tty/objs.a \
		  interrupt/objs.a kernel/objs.a machine/objs.a mm/objs.a \
		  libyrosstd/objs.a $(rust-target-dir)/librust_kernel.a
kernel.exe-LDFLAGS := --whole-archive -m elf_i386 -T kernel.link.ld -static

rust-srcs := $(patsubst ./%,%,$(shell find $(abs_srctree)/macros . -type f -name '*.rs'))

rust-preqs = $(abs_srctree)/Cargo.toml $(abs_srctree)/Cargo.lock
rust-preqs += $(abs_srctree)/i686-none.json
rust-preqs += $(rust-srcs)

rust-output-dir = $(abs_srctree)/target
rust-profile = release
rust-target-triplet = i686-none

rust-target-dir = $(rust-output-dir)/$(rust-target-triplet)/$(rust-profile)

quiet_cmd_cargo = "CARGO	$(2)"
cmd_cargo = cd $(abs_srctree) && CARGO_TARGET_DIR=$(rust-output-dir) cargo +nightly-2026-04-07 $(2)

quiet_cmd_rustc = RUSTC
cmd_rustc = $(call expand_cmd,cargo,rustc) $(RUSTCFLAGS)

RUSTCFLAGS = \
	--manifest-path Cargo.toml \
	--target i686-none.json \
	-Z build-std=core,compiler_builtins,alloc \
	-Z build-std-features=compiler-builtins-mem \
	-Z json-target-spec

ifeq ($(rust-profile),release)
RUSTCFLAGS += --release
endif

$(rust-target-dir)/librust_kernel.a: $(rust-preqs)
	$(call cmd,rustc)

PHONY += rust
rust: $(rust-target-dir)/librust_kernel.a

kernel.bin: kernel.exe
	$(CROSS_COMPILE)objcopy -O binary $< $@

include $(abs_srctree)/scripts/Makefile

CXXFLAGS += -no-pie -fno-rtti -fno-threadsafe-statics -fno-use-cxa-atexit \
	    -DUSE_VESA -DENABLE_SPLASH
