# CROSS_COMPILE
# include-path
# out-bins
# out-libs

MAKE = make

HOSTCC = gcc
HOSTCXX = g++
HOSTLD = g++
HOSTAR = ar

CC = $(CROSS_COMPILE)gcc
CXX = $(CROSS_COMPILE)g++
CPP= $(CROSS_COMPILE)cpp
AS = $(CROSS_COMPILE)as
LD = $(CROSS_COMPILE)ld
AR = $(CROSS_COMPILE)ar

CPPFLAGS += $(foreach d,$(include-path),-I$(d))

CFLAGS = -Wall -g -m32 -nostartfiles -fno-builtin -fno-pic -fno-pie -nostdinc \
	 -nostdlib -fno-stack-protector -fno-unwind-tables \
	 -fno-asynchronous-unwind-tables -fno-exceptions -mno-red-zone \
	 -mno-mmx -mno-sse -mno-sse2 -O0 -Wno-unused-variable \
	 $(CPPFLAGS)

CXXFLAGS = $(CFLAGS) -std=c++20
LDFLAGS =
ARFLAGS = --target=elf32-i386

HOSTCFLAGS = -Wall $(foreach d, $(include-path), -I$(d))
HOSTCXXFLAGS = $(HOSTCFLAGS)
HOSTLDFLAGS =
HOSTARFLAGS =

define BUILD_BIN
$1-hdrs += $$(foreach d, $$(include-path), $$(wildcard $$(d)/*.h) $$(wildcard $$(d)/*.hpp))
$1: $$($1-objs) $$($1-hdrs) $$($1-libs)
	$$(call cmd,ld,$1)
endef

define BUILD_HOST_BIN
$$($1-objs): .%.cpp.cmd: %.cpp
	echo "{\"command\": \"$(HOSTCXX) $(HOSTCXXFLAGS) -c $<\", \"directory\": \"$(realpath $(abs_srctree))\", \"file\": \"$(realpath $<)\"}" > $@

$$($1-objs): %.o: %.cpp .%.cpp.cmd
	$$(call cmd,hostcxx_to_o)

$1-hdrs += $$(foreach d, $$(include-path), $$(wildcard $$(d)/*.h) $$(wildcard $$(d)/*.hpp))
$1: $$($1-objs) $$($1-hdrs) $$($1-libs)
	$$(call cmd,hostld,$1)
endef

define BUILD_LIB
$1-hdrs += $$(foreach d, $$(include-path), $$(wildcard $$(d)/*.h) $$(wildcard $$(d)/*.hpp))
$1: $$($1-objs) $$($1-hdrs) $$($1-libs)
	$$(call cmd,ar,$1-)
endef

out-libnames := $(addprefix lib, $(addsuffix .a, $(out-libs)))

.PRECIOUS: .%.c.cmd .%.cpp.cmd

.%.c.cmd: %.c
	@echo "{\"command\": \"$(CC) $(CFLAGS) -c $<\", \"directory\": \"$(realpath $(abs_srctree))\", \"file\": \"$(realpath $<)\"}" > $@

.%.cpp.cmd: %.cpp
	@echo "{\"command\": \"$(CXX) $(CXXFLAGS) -c $<\", \"directory\": \"$(realpath $(abs_srctree))\", \"file\": \"$(realpath $<)\"}" > $@

%.o: %.c .%.c.cmd
	$(call cmd,cc_to_o)

%.o: %.cpp .%.cpp.cmd
	$(call cmd,cxx_to_o)

host-obj-cmds = $(subst .o,.cpp.cmd,$(host-objs))

$(host-obj-cmds): %.cpp.cmd: %.cpp
	@echo "{\"command\": \"$(HOSTCXX) $(HOSTCXXFLAGS) -c $<\", \"directory\": \"$(realpath $(abs_srctree))\", \"file\": \"$(realpath $<)\"}" > $@

$(host-objs): %.o: %.cpp %.cpp.cmd
	$(call cmd,hostcxx_to_o)

hdrs += $(foreach d,$(include-path),$(wildcard $(d)/*.h) $(wildcard $(d)/*.hpp))
objs.a: $(objs) $(hdrs) $(libs)
	$(call cmd,ar,)

PHONY += dep-hdrs
dep-hdrs: $(hdrs)
	@:

host-objs.a: $(host-objs) $(host-libs) dep-hdrs
	$(call cmd,hostar)

.PHONY: %/objs.a
%/objs.a: %/
	$(call cmd,submake,objs.a)

.PHONY: %/host-objs.a
%/host-objs.a: %/
	$(call cmd,submake,host-objs.a)

.PHONY: %/objs.a.clean
%/objs.a.clean: %/
	$(call cmd,submake_clean)

.PHONY: %/host-objs.a.clean
%/host-objs.a.clean: %/
	$(call cmd,submake_clean)

.PHONY: %.clean
%.clean:
	$(call cmd,rm,$(patsubst %.clean,%,$@))

$(foreach bin, $(out-bins), $(eval $(call BUILD_BIN,$(bin))))
$(foreach bin, $(out-host-bins), $(eval $(call BUILD_HOST_BIN,$(bin))))
$(foreach lib, $(out-libnames), $(eval $(call BUILD_LIB,$(lib))))

clean_files = $(foreach item,$(1),$(call cmd,rm,$(item)))

PHONY += clean custom-clean
clean: $(addsuffix .clean,\
		$(filter %/objs.a, $(foreach bin,$(out-bins),$($(bin)-libs))) \
		$(filter %/host-objs.a, $(foreach bin,$(out-host-bins),$($(bin)-libs))) \
		$(foreach bin,$(out-bins),$($(bin)-objs)) \
		$(foreach bin,$(out-host-bins),$($(bin)-objs)) \
		$(foreach lib,$(out-libnames),$($(lib)-objs)) \
		$(out-bins) $(out-host-bins) $(out-libnames) \
		objs.a $(objs) host-objs.a $(host-objs) \
		$(wildcard **/*.cmd **/.*.cmd) collect-commands.cmd \
	) custom-clean

.PHONY: all
all: $(out-libnames) $(out-bins) $(out-host-bins)

.PHONY: %/collect-commands.cmd
%/collect-commands.cmd: %/
	$(call cmd,submake,collect-commands.cmd)

collect-commands.cmd: $(subst objs.a,collect-commands.cmd,$(filter %/objs.a, $(libs))) \
	$(subst objs.a,collect-commands.cmd,$(filter %/objs.a, $(foreach bin,$(out-bins),$($(bin)-libs)))) \
	$(subst host-objs.a,collect-commands.cmd,$(filter %/host-objs.a, $(foreach bin,$(out-host-bins),$($(bin)-libs))))
	sh -c "cat .*.cmd **/*.cmd > $@ || true"
	- [ "$^" ] && cat $^ >> $@

include $(abs_srctree)/scripts/Makefile.lib

.DEFAULT_GOAL = all
