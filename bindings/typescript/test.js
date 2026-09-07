const assert = require("node:assert");
const { hello } = require("./index.js");

const result = hello();
assert.strictEqual(result, "z3rno bindings scaffold OK");
console.log(result);
