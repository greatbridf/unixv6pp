.DEFAULT_GOAL := all

MAKEFLAGS += -r

QEMU = qemu-system-i386

QEMU_FLAGS := -m 64M \
	      -rtc base=localtime \
	      -d cpu_reset -D target/qemu.log \
	      -machine pc -cpu Icelake-Server \
	      -serial mon:stdio

QEMU_GDB_FLAGS := -chardev socket,path=target/qemu-gdb.sock,server=on,wait=off,id=gdb0 \
		  -gdb chardev:gdb0 -S

QEMU_DISK := -boot c -drive file=target/disk.img,if=ide,index=0,media=disk,format=raw

this-makefile := $(lastword $(MAKEFILE_LIST))
export abs_srctree := $(realpath $(dir $(this-makefile)))
export abs_objtree := $(CURDIR)

target-dirs := tools lib shell programs src
build-dirs := $(addprefix build-,$(target-dirs))
clean-dirs := $(addprefix clean-,$(target-dirs))

PHONY += $(build-dirs)
PHONY += $(clean-dirs)

$(build-dirs): build-%: %
	$(call cmd,submake,)

$(clean-dirs): clean-%: %
	$(call cmd,submake,clean)

.PHONY: install-buildrequires
install-buildrequires:
	brew install gmake x86_64-elf-gcc nasm

.PHONY: clean
clean: $(clean-dirs)
	-$(call cmd,rmdir,$(workdir))

workdir := $(realpath $(CURDIR))/target/img-workspace

PHONY += build-rust
build-rust: src
	$(call cmd,submake,rust)

$(workdir)/kernel.bin: src FORCE
	$(call cmd,mkdir,$(workdir))
	$(call cmd,submake,kernel.bin)
	$(call cmd,cp,src/kernel.bin,$@)

$(workdir)/boot.bin: src/boot FORCE
	$(call cmd,mkdir,$(workdir))
	$(call cmd,submake,boot.bin)
	$(call cmd,cp,src/boot/boot.bin,$@)

PHONY += install-programs install-shell
install-programs: programs FORCE
	$(call cmd,mkdir,$(workdir)/programs/bin)
	$(call cmd,submake,install INSTALL_PATH=$(workdir)/programs/bin)

install-shell: build-shell shell/Shell.exe FORCE
	$(call cmd,mkdir,$(workdir)/programs)
	$(call cmd,cp,$(call filter-out-phony,$^),$(workdir)/programs/)

target/disk.img: $(build-dirs) $(workdir)/kernel.bin $(workdir)/boot.bin \
		install-programs install-shell
	$(call cmd,mkdir,$(workdir)/programs/etc)
	$(call cmd,cp,tools/unixv6pp_splash/v6pp_splash.bmp,$(workdir)/programs/etc/)
	$(Q)cd $(workdir)/ && $(abs_srctree)/tools/filescanner \
		| $(abs_srctree)/tools/fs-editor \
		$(abs_srctree)/target/disk.img c

.PHONY: qemu
qemu: target/disk.img
	$(QEMU) $(QEMU_FLAGS) $(QEMU_DISK)

.PHONY: qemug
qemug: target/disk.img
	$(QEMU) $(QEMU_FLAGS) $(QEMU_DISK) $(QEMU_GDB_FLAGS)

.PHONY: all
all: $(build-dirs) target/disk.img

cmd_compile_commands = make -C $(1) collect-commands.cmd \
	&& printf "[" > $@ \
	&& paste "-d," -s $(1)/collect-commands.cmd >> $@ \
	&& printf "]" >> $@

tools/compile_commands.json: build-tools
	$(call cmd_compile_commands,tools)

compile_commands.json: all
	$(call cmd_compile_commands,src)

PHONY += cargo
cargo: src
	$(call cmd,submake,cargo)

PHONY += debug
debug:
	-@RUST_GDB=x86_64-elf-gdb rust-gdb --symbols=src/kernel.exe \
		-iex 'set pagination off' \
		-iex 'set output-radix 16' \
		-iex 'set print asm-demangle on' \
		-iex 'set print pretty on' \
		-iex 'target remote target/qemu-gdb.sock' \
		$(foreach bp,$(B),-ex 'b $(bp)')

include $(abs_srctree)/scripts/Makefile.lib
