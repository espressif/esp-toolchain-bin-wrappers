CROSS_COMPILE ?= ""
CC = $(CROSS_COMPILE)gcc
CXX = $(CROSS_COMPILE)g++
AR = $(CROSS_COMPILE)ar

RELEASE_FLAGS = -O2

esp-elf-gdb-wrapper.exe:
	$(CC) ${RELEASE_FLAGS} -DTARGET_ESP_ARCH_${TARGET_ESP_ARCH} main.c -o $@

install: esp-elf-gdb-wrapper.exe
	mkdir -p $(DESTDIR)$(PREFIX)/bin
	cp esp-elf-gdb-wrapper.exe $(DESTDIR)$(PREFIX)/bin

clean:
	rm -f esp-elf-gdb-wrapper.exe
