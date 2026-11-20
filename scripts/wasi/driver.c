/*
 * Command-style test driver for the WASI (Wasm MVP) CSPICE build.
 *
 * Hosts that only run `_start` (for example `spacewasm run`, or `wasmtime run`) use
 * this instead of the reactor module. It furnishes a leapseconds kernel and an SPK
 * from a preopened directory, converts one UTC string to ET, and evaluates one
 * spkezr_c state, printing every number with 17 significant digits so the output can
 * be compared against the Emscripten build.
 *
 * Usage: cspice-driver.wasi.wasm <lsk path> <spk path> [repeat]
 * With repeat > 1 the spkezr_c call is repeated at et + k * 3600 s, which gives a
 * host a steady-state loop to time.
 */
#include <stdio.h>
#include <stdlib.h>

#include "SpiceUsr.h"

static int check(const char *what) {
  if (failed_c()) {
    SpiceChar msg[1841];
    getmsg_c("LONG", (SpiceInt)sizeof msg, msg);
    printf("error in %s: %s\n", what, msg);
    return 1;
  }
  return 0;
}

int main(int argc, char **argv) {
  if (argc < 3) {
    printf("usage: %s <lsk> <spk> [repeat]\n", argv[0]);
    return 2;
  }
  int repeat = argc > 3 ? atoi(argv[3]) : 1;

  erract_c("SET", 0, "RETURN");
  errprt_c("SET", 0, "NONE");

  furnsh_c(argv[1]);
  if (check("furnsh_c(lsk)")) return 1;
  furnsh_c(argv[2]);
  if (check("furnsh_c(spk)")) return 1;

  SpiceDouble et = 0.0;
  str2et_c("2004-07-01T00:00:00", &et);
  if (check("str2et_c")) return 1;
  printf("et %.17g\n", et);

  SpiceDouble state[6], lt = 0.0;
  for (int k = 0; k < repeat; k++) {
    spkezr_c("EARTH", et + 3600.0 * k, "J2000", "LT+S", "SUN", state, &lt);
    if (check("spkezr_c")) return 1;
  }
  printf("state %.17g %.17g %.17g %.17g %.17g %.17g\n", state[0], state[1], state[2], state[3],
         state[4], state[5]);
  printf("lt %.17g\n", lt);
  return 0;
}
