/* TypeScript stress fixture.
 * The comment spans multiple lines and contains non-ASCII: café λ🚀.
 */

type Pair<T extends string | number> = {
  left: T;
  right?: T;
};

const route = "/api/v1/café";
const routePattern = /^\/api\/(?<version>v\d+)\/[\p{Letter}-]+$/u;
const quotedAttribute = /(\w+)\s*=\s*(["']).*?\2/g;

const total = 42;
const count = 7;
const ratio = total / count / 2;
const regexThenDivision = routePattern.test(route) ? total / /\d+/.source.length : 0;

const message = `multi-line template
route=${route}
match=${routePattern.test(route) ? "✓" : "✗"}`;

export function pick<T extends string | number>(pair: Pair<T>): T {
  return pair.right ?? pair.left;
}

export const summary = `${pick({ left: "λ", right: "🚀" })}: ${message} ${ratio}`;
