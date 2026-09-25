// Line keys: small SVG samples of a curve's stroke for legends and readouts.

import { DASHES, styleSlot } from './series';

export function svgKey(stroke: string, dash: number[], width: number, w = 28): SVGSVGElement {
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('width', String(w));
  svg.setAttribute('height', '10');
  svg.setAttribute('aria-hidden', 'true');
  svg.classList.add('line-key');
  const line = document.createElementNS(ns, 'line');
  line.setAttribute('x1', '0');
  line.setAttribute('x2', String(w));
  line.setAttribute('y1', '5');
  line.setAttribute('y2', '5');
  line.setAttribute('style', `stroke: ${stroke}; stroke-width: ${width}`);
  if (dash.length) line.setAttribute('stroke-dasharray', dash.join(' '));
  svg.append(line);
  return svg;
}

/** The key of a live probe's curve: its colour and dash. */
export function lineKey(probe: number, width = 28): SVGSVGElement {
  const st = styleSlot(probe);
  return svgKey(`var(--series-${st.color + 1})`, DASHES[st.dash], 2.5, width);
}
