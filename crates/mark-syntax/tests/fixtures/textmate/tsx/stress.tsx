import React from "react";

type Props = {
  title: string;
  items: string[];
};

export function StressPanel({ title, items }: Props) {
  const ratio = items.length / Math.max(title.length, 1);
  const hasCapital = /[A-Z][\w-]+/.test(title);

  return (
    <section data-title={title} data-state={hasCapital ? "open" : "closed"} aria-label={`Δ ${title}`}>
      {/* JSX comment with non-ASCII text: café λ🚀 */}
      <header className="panel__header">
        <h1>{title}</h1>
        <span data-ratio={ratio / 2}>{hasCapital ? "Regex" : "division"}</span>
      </header>
      <input disabled={ratio > 1} value={items.join(" / ")} readOnly />
      <ul>
        {items.map((item, index) => (
          <li key={`${item}-${index}`} data-index={index}>
            <button onClick={() => console.log(/ok\/(done)?/u.test(item))}>
              {item.toUpperCase()}
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}
