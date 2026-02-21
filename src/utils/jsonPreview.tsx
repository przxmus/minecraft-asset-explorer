import type { ReactElement } from "react";

export function decodePreviewJson(base64: string): string {
  try {
    const binary = atob(base64);
    const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
    const raw = new TextDecoder().decode(bytes);
    const parsed = JSON.parse(raw);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return "Invalid JSON content.";
  }
}

export function renderHighlightedJson(value: string): ReactElement[] {
  const tokenRegex =
    /(\"(?:\\u[a-fA-F0-9]{4}|\\[^u]|[^\\\"])*\"\s*:?)|\\b(true|false|null)\\b|-?\\d+(?:\\.\\d+)?(?:[eE][+\\-]?\\d+)?/g;

  const nodes: ReactElement[] = [];
  let lastIndex = 0;
  let match: RegExpExecArray | null = tokenRegex.exec(value);
  let key = 0;

  while (match) {
    const token = match[0];
    const start = match.index;

    if (start > lastIndex) {
      nodes.push(
        <span className="json-punctuation" key={`plain-${key++}`}>
          {value.slice(lastIndex, start)}
        </span>,
      );
    }

    let className = "json-number";
    if (/\"\s*:$/.test(token)) {
      className = "json-key";
    } else if (token.startsWith('"')) {
      className = "json-string";
    } else if (/^(true|false|null)$/.test(token)) {
      className = "json-literal";
    }

    nodes.push(
      <span className={className} key={`tok-${key++}`}>
        {token}
      </span>,
    );

    lastIndex = start + token.length;
    match = tokenRegex.exec(value);
  }

  if (lastIndex < value.length) {
    nodes.push(
      <span className="json-punctuation" key={`plain-${key++}`}>
        {value.slice(lastIndex)}
      </span>,
    );
  }

  return nodes;
}
