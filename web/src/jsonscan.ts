// Position-tracking JSON scanner and surgical parameter rewrites.
//
// The netlist text is the single source of truth (spec Section 2,
// principle 1). A design control never re-serialises the document: it
// replaces exactly the characters of one value token, so the user's
// formatting, key order, spacing and every other value stay as written.
//
// The scanner is a strict RFC 8259 parser (as strict as the engine's
// serde_json: no comments, no trailing commas, no leading zeros) that
// records the character range of every value and key. Duplicate keys
// resolve to the last occurrence, as they do in the engine.

import type { Scalar } from './types';

export interface Span {
  /** Offset of the first character. */
  start: number;
  /** Offset one past the last character. */
  end: number;
}

export type JNode =
  | (Span & { kind: 'object'; members: JMember[] })
  | (Span & { kind: 'array'; items: JNode[] })
  | (Span & { kind: 'string'; value: string })
  | (Span & { kind: 'number'; value: number })
  | (Span & { kind: 'boolean'; value: boolean })
  | (Span & { kind: 'null' });

export interface JMember {
  key: string;
  /** Range of the key token, quotes included. */
  keySpan: Span;
  value: JNode;
}

export class JsonScanError extends Error {
  constructor(
    message: string,
    public readonly offset: number,
  ) {
    super(message);
  }
}

/** Parses `text`; throws JsonScanError with the offset of the first error. */
export function scan(text: string): JNode {
  let i = 0;
  let depth = 0;
  const fail = (msg: string): never => {
    throw new JsonScanError(msg, i);
  };
  const ws = () => {
    while (i < text.length) {
      const c = text.charCodeAt(i);
      if (c === 0x20 || c === 0x09 || c === 0x0a || c === 0x0d) i++;
      else break;
    }
  };
  const str = (): string => {
    // text[i] is the opening quote.
    i++;
    let out = '';
    let run = i;
    for (;;) {
      if (i >= text.length) fail('unterminated string');
      const c = text.charCodeAt(i);
      if (c === 0x22) {
        out += text.slice(run, i);
        i++;
        return out;
      }
      if (c < 0x20) fail('control character in string');
      if (c === 0x5c) {
        out += text.slice(run, i);
        const e = text[i + 1];
        const simple: Record<string, string> = { '"': '"', '\\': '\\', '/': '/', b: '\b', f: '\f', n: '\n', r: '\r', t: '\t' };
        if (e in simple) {
          out += simple[e];
          i += 2;
        } else if (e === 'u') {
          const hex4 = (at: number) => {
            const hex = text.slice(at, at + 4);
            return /^[0-9a-fA-F]{4}$/.test(hex) ? parseInt(hex, 16) : -1;
          };
          const u = hex4(i + 2);
          if (u < 0) fail('bad \\u escape');
          i += 6;
          // A surrogate must come as an escaped high-low pair (serde_json
          // rejects a lone one, so the engine would reject the text).
          if (u >= 0xdc00 && u <= 0xdfff) fail('lone low surrogate in \\u escape');
          if (u >= 0xd800 && u <= 0xdbff) {
            const lo = text[i] === '\\' && text[i + 1] === 'u' ? hex4(i + 2) : -1;
            if (lo < 0xdc00 || lo > 0xdfff) fail('lone high surrogate in \\u escape');
            out += String.fromCharCode(u, lo);
            i += 6;
          } else {
            out += String.fromCharCode(u);
          }
        } else {
          i++;
          fail('bad escape');
        }
        run = i;
        continue;
      }
      i++;
    }
  };
  const NUM = /-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/y;
  const value = (): JNode => {
    ws();
    const start = i;
    const c = text[i];
    if (c === '{') {
      if (++depth > 128) fail('nested too deeply');
      i++;
      const members: JMember[] = [];
      ws();
      if (text[i] === '}') {
        i++;
      } else {
        for (;;) {
          ws();
          if (text[i] !== '"') fail('expected a key');
          const ks = i;
          const key = str();
          const keySpan = { start: ks, end: i };
          ws();
          if (text[i] !== ':') fail("expected ':'");
          i++;
          members.push({ key, keySpan, value: value() });
          ws();
          if (text[i] === ',') {
            i++;
            continue;
          }
          if (text[i] === '}') {
            i++;
            break;
          }
          fail("expected ',' or '}'");
        }
      }
      depth--;
      return { kind: 'object', start, end: i, members };
    }
    if (c === '[') {
      if (++depth > 128) fail('nested too deeply');
      i++;
      const items: JNode[] = [];
      ws();
      if (text[i] === ']') {
        i++;
      } else {
        for (;;) {
          items.push(value());
          ws();
          if (text[i] === ',') {
            i++;
            continue;
          }
          if (text[i] === ']') {
            i++;
            break;
          }
          fail("expected ',' or ']'");
        }
      }
      depth--;
      return { kind: 'array', start, end: i, items };
    }
    if (c === '"') return { kind: 'string', start, value: str(), end: i };
    for (const [word, node] of [
      ['true', { kind: 'boolean', value: true }],
      ['false', { kind: 'boolean', value: false }],
      ['null', { kind: 'null' }],
    ] as const) {
      if (text.startsWith(word, i)) {
        i += word.length;
        return { ...node, start, end: i } as JNode;
      }
    }
    NUM.lastIndex = i;
    const m = NUM.exec(text);
    if (m && m[0].length) {
      i += m[0].length;
      const v = Number(m[0]);
      if (!Number.isFinite(v)) fail('number out of range');
      return { kind: 'number', start, end: i, value: v };
    }
    return fail(i >= text.length ? 'unexpected end of text' : 'unexpected character');
  };
  const root = value();
  ws();
  if (i < text.length) fail('unexpected text after the document');
  return root;
}

/** Last member named `key` of an object node (duplicates: last wins). */
export function member(node: JNode | undefined, key: string): JMember | undefined {
  if (node?.kind !== 'object') return undefined;
  for (let k = node.members.length - 1; k >= 0; k--) if (node.members[k].key === key) return node.members[k];
  return undefined;
}

/** JSON text of a scalar. Numbers must be finite. */
export function scalarText(v: Scalar): string {
  if (typeof v === 'number' && !Number.isFinite(v)) throw new Error(`${v} is not a JSON number`);
  return JSON.stringify(v);
}

/**
 * The value token of parameter `name`: the whole entry for the shorthand
 * `"name": 3`, or the `value` member of `"name": {"value": 3, ...}`. Null if
 * the parameter is not declared or has no literal value (a derived one).
 */
export function paramValueNode(root: JNode, name: string): JNode | null {
  const p = member(member(root, 'parameters')?.value, name)?.value;
  if (!p) return null;
  if (p.kind === 'object') {
    const v = member(p, 'value')?.value;
    return v && v.kind !== 'object' && v.kind !== 'array' && v.kind !== 'null' ? v : null;
  }
  return p.kind === 'array' || p.kind === 'null' ? null : p;
}

/** Range of a parameter's declaration, `"name": ...` (for "show in netlist"). */
export function paramDeclSpan(root: JNode, name: string): Span | null {
  const m = member(member(root, 'parameters')?.value, name);
  return m ? { start: m.keySpan.start, end: m.value.end } : null;
}

/**
 * Returns `text` with parameter `name`'s value replaced by `value`, every
 * other character unchanged. Throws if the text is not valid JSON or the
 * parameter has no literal value to replace.
 */
export function setParam(text: string, name: string, value: Scalar): string {
  return setParams(text, [[name, value]]);
}

/**
 * As `setParam` for several parameters at once (edits applied back to
 * front). A name given twice takes its last value: two edits of one token
 * would otherwise splice the second into the first's leftovers.
 */
export function setParams(text: string, values: [string, Scalar][]): string {
  const root = scan(text);
  const edits = [...new Map(values).entries()].map(([name, v]) => {
    const node = paramValueNode(root, name);
    if (!node) throw new Error(`parameter '${name}' has no value in the netlist text`);
    return { ...node, text: scalarText(v) } as Span & { text: string };
  });
  edits.sort((a, b) => b.start - a.start);
  for (let k = 1; k < edits.length; k++) {
    if (edits[k].end > edits[k - 1].start) throw new Error('overlapping parameter values in the netlist text');
  }
  let out = text;
  for (const e of edits) out = out.slice(0, e.start) + e.text + out.slice(e.end);
  return out;
}

/** Literal (non-derived) parameter values declared in a netlist text, by name. */
export function declaredValues(text: string): Map<string, Scalar> {
  const out = new Map<string, Scalar>();
  let root: JNode;
  try {
    root = scan(text);
  } catch {
    return out;
  }
  const params = member(root, 'parameters')?.value;
  if (params?.kind !== 'object') return out;
  for (const m of params.members) {
    const v = paramValueNode(root, m.key);
    if (v && (v.kind === 'number' || v.kind === 'boolean' || v.kind === 'string')) out.set(m.key, v.value);
  }
  return out;
}
