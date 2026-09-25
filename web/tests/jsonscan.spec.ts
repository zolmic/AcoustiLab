// Unit tests of the position-tracking JSON scanner behind the design
// panel's surgical rewrites (no browser needed). The oracle for parsing is
// JSON.parse. For rewriting it is the exact expected text: each case names
// the old token by a context that occurs once in the input, and the output
// must be the input with that token, and nothing else, replaced. The output
// must also parse to the input document with exactly that value changed.

import { expect, test } from '@playwright/test';
import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { declaredValues, JsonScanError, paramDeclSpan, scan, setParam, setParams, type JNode } from '../src/jsonscan';

const repo = fileURLToPath(new URL('../../', import.meta.url));

/** Plain value of a scanned node (to compare with JSON.parse). */
function plain(n: JNode): unknown {
  switch (n.kind) {
    case 'object': {
      const o: Record<string, unknown> = {};
      for (const m of n.members) o[m.key] = plain(m.value);
      return o;
    }
    case 'array':
      return n.items.map(plain);
    case 'null':
      return null;
    default:
      return n.value;
  }
}

/** Spans are consistent: every node's text slice parses to its value. */
function checkSpans(text: string, n: JNode): void {
  expect(JSON.parse(text.slice(n.start, n.end))).toEqual(plain(n));
  if (n.kind === 'object') {
    for (const m of n.members) {
      expect(JSON.parse(text.slice(m.keySpan.start, m.keySpan.end))).toBe(m.key);
      checkSpans(text, m.value);
    }
  } else if (n.kind === 'array') {
    for (const it of n.items) checkSpans(text, it);
  }
}

test('scan agrees with JSON.parse on every example netlist, with exact spans', () => {
  const dir = `${repo}/examples`;
  const files = readdirSync(dir).filter((f) => f.endsWith('.json'));
  expect(files.length).toBeGreaterThan(3);
  for (const f of files) {
    const text = readFileSync(`${dir}/${f}`, 'utf8');
    const root = scan(text);
    expect(plain(root), f).toEqual(JSON.parse(text));
    checkSpans(text, root);
  }
});

test('scan accepts what JSON.parse accepts and rejects what it rejects', () => {
  const good = [
    '{}',
    '[]',
    ' {"a" : [1, -2.5e-3, 0, 1E+2, true, false, null, "x\\"y\\\\z\\u00e9\\n"]}\r\n',
    '{"a":{"b":{"c":[[[]]]}}}',
    '"just a string"',
    '-0',
    '{"a":1,"a":2}',
    '{"\\u0061b": 1}',
  ];
  for (const t of good) expect(plain(scan(t)), t).toEqual(JSON.parse(t));
  const bad = [
    '',
    '{',
    '{"a":1,}',
    '[1,]',
    '{"a":01}',
    '{"a":.5}',
    '{"a":1.}',
    '{a:1}',
    "{'a':1}",
    '{"a":1} x',
    '// c\n{}',
    '{"a":"tab\there"}',
    '{"a":"\\x"}',
    '{"a":NaN}',
    '{"a":tru}',
    '[1 2]',
  ];
  for (const t of bad) {
    expect(() => JSON.parse(t), t).toThrow();
    expect(() => scan(t), t).toThrow(JsonScanError);
  }
  // The error offset points at the fault.
  const t = '{\n  "a": 1,\n  "b": ,\n}';
  expect(() => scan(t)).toThrow(JsonScanError);
  try {
    scan(t);
  } catch (e) {
    expect((e as JsonScanError).offset).toBe('{\n  "a": 1,\n  "b": '.length);
  }
});

test('setParam replaces exactly one value token, whatever the formatting', () => {
  // `ctx` is the text just before the old token; ctx + old occurs once
  // (for the duplicate-key case, the last occurrence is meant).
  const cases: {
    name: string;
    text: string;
    param: string;
    value: number | boolean | string;
    ctx: string;
    token: [string, string];
    last?: boolean;
  }[] = [
    {
      name: 'compact, object form',
      text: '{"parameters":{"n":{"value":1,"min":0,"max":12,"integer":true}},"elements":[{"count":"=n"}]}',
      param: 'n',
      value: 3,
      ctx: '{"value":',
      token: ['1', '3'],
    },
    {
      name: 'pretty, value after other keys',
      text: '{\n  "parameters": {\n    "d_mm": {\n      "min": 0.3,\n      "max": 8,\n      "value": 3\n    }\n  }\n}\n',
      param: 'd_mm',
      value: 2.5,
      ctx: '"value": ',
      token: ['3', '2.5'],
    },
    {
      name: 'shorthand number',
      text: '{"parameters": {"gap_mm": 0.08, "w_mm": 15}}',
      param: 'gap_mm',
      value: 0.12,
      ctx: '"gap_mm": ',
      token: ['0.08', '0.12'],
    },
    {
      name: 'shorthand boolean',
      text: '{"parameters": {"sealed": false}}',
      param: 'sealed',
      value: true,
      ctx: '"sealed": ',
      token: ['false', 'true'],
    },
    {
      name: 'shorthand string',
      text: '{"parameters": {"ear": "iec60318_4"}}',
      param: 'ear',
      value: 'type43',
      ctx: '"ear": ',
      token: ['"iec60318_4"', '"type43"'],
    },
    {
      name: 'choice: the nested choices\' "value" keys are left alone',
      text: '{"parameters": {"ear": {"choices": [{"value": "a", "label": "A"}, {"value": "b"}], "value": "a"}}}',
      param: 'ear',
      value: 'b',
      ctx: '}], "value": ',
      token: ['"a"', '"b"'],
    },
    {
      name: 'tabs, CRLF and non-ASCII text before the value',
      text: '{\r\n\t"title": "Kopfhörer é ☃ 𝄞",\r\n\t"parameters": {\r\n\t\t"x_mm": {\t"value"\t:\t-1.5e1 }\r\n\t}\r\n}',
      param: 'x_mm',
      value: 7,
      ctx: '"value"\t:\t',
      token: ['-1.5e1', '7'],
    },
    {
      name: 'a description quoting a value, and the same key elsewhere',
      text: '{"description": "\\"value\\": 5", "n": 5, "parameters": {"n": {"description": "\\"value\\": 5", "value": 5}}, "elements": [{"n": 5}]}',
      param: 'n',
      value: 6,
      ctx: '5", "value": ',
      token: ['5', '6'],
    },
    {
      name: 'duplicate declaration: the last one, as in the engine',
      text: '{"parameters": {"n": 1, "n": 2}}',
      param: 'n',
      value: 9,
      ctx: '"n": ',
      token: ['2', '9'],
      last: true,
    },
    {
      name: 'escaped key spelling',
      text: '{"parameters": {"a\\u005fb": 1}}',
      param: 'a_b',
      value: 4,
      ctx: '"a\\u005fb": ',
      token: ['1', '4'],
    },
    {
      name: 'small numbers stay valid JSON',
      text: '{"parameters": {"p": 1}}',
      param: 'p',
      value: 1e-7,
      ctx: '"p": ',
      token: ['1', '1e-7'],
    },
  ];
  for (const c of cases) {
    const needle = c.ctx + c.token[0];
    const at = c.last ? c.text.lastIndexOf(needle) : c.text.indexOf(needle);
    expect(at, c.name).toBeGreaterThanOrEqual(0);
    if (!c.last) expect(c.text.lastIndexOf(needle), `${c.name}: context is unique`).toBe(at);
    const k = at + c.ctx.length;
    const out = setParam(c.text, c.param, c.value);
    expect(out, c.name).toBe(c.text.slice(0, k) + c.token[1] + c.text.slice(k + c.token[0].length));

    // Parsed, only that parameter's value differs.
    const want = JSON.parse(c.text);
    const got = JSON.parse(out);
    const p = got.parameters[c.param];
    expect(typeof p === 'object' && p !== null ? p.value : p, c.name).toEqual(c.value);
    const strip = (d: { parameters: Record<string, unknown> }) => {
      const q = d.parameters[c.param];
      if (typeof q === 'object' && q !== null) delete (q as Record<string, unknown>).value;
      else delete d.parameters[c.param];
      return d;
    };
    expect(strip(got), c.name).toEqual(strip(want));
  }
});

test('setParam refuses what it cannot rewrite', () => {
  expect(() => setParam('{"parameters": {"v_cm3": {"expr": "2*3"}}}', 'v_cm3', 1)).toThrow(/no value/);
  expect(() => setParam('{"parameters": {"a": 1}}', 'b', 1)).toThrow(/no value/);
  expect(() => setParam('{"a": 1}', 'a', 2)).toThrow(/no value/);
  expect(() => setParam('{"parameters": {"a": 1,}}', 'a', 2)).toThrow(JsonScanError);
  expect(() => setParam('{"parameters": {"a": 1}}', 'a', Number.NaN)).toThrow(/not a JSON number/);
  expect(() => setParam('{"parameters": {"a": {"value": [1]}}}', 'a', 2)).toThrow(/no value/);
});

test('several parameters at once, on the design template', () => {
  const text = readFileSync(`${repo}/examples/design_over_ear.json`, 'utf8');
  const out = setParams(text, [
    ['vent_count', 3],
    ['rear', 'open'],
    ['front_depth_mm', 12.5],
  ]);
  const a = JSON.parse(text);
  const b = JSON.parse(out);
  a.parameters.vent_count.value = 3;
  a.parameters.rear.value = 'open';
  a.parameters.front_depth_mm.value = 12.5;
  expect(b).toEqual(a);
  // Three lines change, each only in its value token.
  const before = text.split('\n');
  const after = out.split('\n');
  expect(after.length).toBe(before.length);
  const changed = after.map((l, k) => [before[k], l]).filter(([x, y]) => x !== y);
  expect(changed.map(([x, y]) => [x.replace(/"value": [^,}]+/, ''), y.replace(/"value": [^,}]+/, '')])).toEqual(
    changed.map(([x]) => [x.replace(/"value": [^,}]+/, ''), x.replace(/"value": [^,}]+/, '')]),
  );
  expect(changed).toHaveLength(3);

  const declared = declaredValues(text);
  expect(declared.get('vent_count')).toBe(1);
  expect(declared.get('ear')).toBe('iec60318_4');
  expect(declared.has('front_volume_cm3')).toBe(false);
  const span = paramDeclSpan(scan(text), 'vent_count')!;
  expect(text.slice(span.start, span.end)).toMatch(/^"vent_count": \{"value": 1,.*"group": "Rear"\}$/s);
});
