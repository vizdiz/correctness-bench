#!/bin/sh
# Clone wrk2, swap the bundled LuaJIT 2.0 for upstream LuaJIT 2.1 (arm64-capable),
# patch the two source incompatibilities introduced by that swap, and build.
set -eu

git clone --depth 1 https://github.com/giltene/wrk2.git .

# LuaJIT 2.0 -> 2.1.
rm -rf deps/luajit
git clone --depth 1 --branch v2.1 https://github.com/LuaJIT/LuaJIT.git deps/luajit

# LuaJIT 2.1 renamed struct luaL_reg -> luaL_Reg.
sed -i 's/struct luaL_reg/struct luaL_Reg/g; s/luaL_reg /luaL_Reg /g' src/script.c

# wrk2's vendored hdr_histogram hardcodes <x86intrin.h>; the intrinsics it
# would provide are only used on x86. Guard the include so arm64 builds.
python3 - <<'PY'
path = 'src/hdr_histogram.c'
src = open(path).read()
needle = '#include <x86intrin.h>'
guarded = ('#if defined(__x86_64__) || defined(__i386__)\n'
           '#include <x86intrin.h>\n'
           '#endif')
if needle not in src:
    raise SystemExit('drift: x86intrin include not found verbatim')
open(path, 'w').write(src.replace(needle, guarded, 1))
PY

make WITH_OPENSSL=/usr
