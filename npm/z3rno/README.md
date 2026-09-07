# z3rno

The `z3rno` CLI, distributed as a prebuilt native binary via npm.

```sh
npx z3rno init
```

This package is a thin wrapper: it has no logic of its own beyond picking
the right platform binary at runtime from one of the `@z3rno/cli-*`
optional dependencies (see `bin/z3rno.js`). No Rust toolchain required.
