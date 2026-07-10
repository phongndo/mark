/* JavaScript stress fixture.
 * Multi-line comment with non-ASCII text: café λ🚀.
 */

const path = "/api/v1/café";
const routePattern = /^\/api\/(?<version>v\d+)\/[\p{Letter}-]+$/u;
const attributePattern = /(\w+)\s*=\s*(["']).*?\2/g;

const total = 84;
const count = 6;
const ratio = total / count / 2;
const divideByRegexSourceLength = total / /[0-9]+/.source.length;

const replacement = `multi-line template
route=${path}
match=${routePattern.test(path) ? "✓" : "✗"}`;

function normalize(value) {
  return value.replace(attributePattern, "$1=…").trim() / 2;
}

export { routePattern, ratio, divideByRegexSourceLength, normalize, replacement };
