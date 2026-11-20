/*
 * Shims for the WASI (Wasm MVP) build of CSPICE.
 *
 * wasi-libc has no tmpfile(), and the libf2c runtime inside CSPICE references it from
 * the Fortran scratch-file paths (endfile.c, open.c). SPICE kernel loading and the
 * exported API never open scratch units, so this stub reports failure instead of
 * creating a file: the f2c caller sees a NULL stream and raises its usual I/O error.
 */
#include <errno.h>
#include <stdio.h>

FILE *tmpfile(void) {
  errno = ENOSYS;
  return NULL;
}
