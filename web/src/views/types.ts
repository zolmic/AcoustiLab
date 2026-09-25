// Result views: tabs of the results panel beyond "Response" (sensitivity,
// tolerance, target, time, isolation, fit, listen...). Each view is a module
// `src/views/<name>.view.ts` exporting `view: ResultView`; `registry.ts`
// finds them with import.meta.glob, so adding a view touches no shared file.

import type { EngineWorker, Reply } from '../engine';
import type { ParamDesc, ParamsDoc, Scalar, SolveResult } from '../types';

export type { ParamDesc, ParamsDoc, Scalar, SolveResult };

/** What a view can read and do. Everything is derived from the netlist text. */
export interface ViewHost {
  /** The current netlist text (the single source of truth). */
  netlist(): string;
  /** The latest successful live solve and the text it was solved from. */
  current(): { result: SolveResult; text: string } | null;
  /**
   * The parameter descriptions of the current text (the `parameters()`
   * export's document), or null when the netlist declares none.
   */
  parameters(): ParamsDoc | null;
  /**
   * Declared parameter values of the loaded template (what "Reset" restores),
   * or null when the text was not loaded from a template.
   */
  reference(): Map<string, Scalar> | null;
  /** Frozen baselines (Response tab), each with the netlist text it was solved from. */
  baselines(): { id: number; name: string; text: string }[];
  /** Id of the baseline chosen as the Δ reference, if any. */
  deltaReference(): number | null;
  /**
   * Calls a wasm export by name on a worker reserved for this view, so long
   * jobs never delay the live solve. Arguments are JSON text.
   */
  call(fn: string, ...args: string[]): Promise<Reply>;
  /** Terminates this view's worker (cancelling its running call). */
  cancel(): void;
  /** Writes parameter values into the netlist text, as the design controls do (then re-solves). */
  setParameters(values: [string, Scalar][]): void;
  /** Announces a short status message to assistive technology. */
  announce(message: string): void;
  /**
   * Picks out a frequency range on the Response plots (as a selected warning
   * does; null clears it). The next solve's warnings replace it.
   */
  highlight(range: { lo: number; hi: number; label: string } | null): void;
  /** Shows the Design tab and focuses the control of this parameter. */
  focusParameter(name: string): void;
}

export interface ResultView {
  /** Stable id: the tab's DOM id is `view-tab-<id>`, its panel `view-<id>`. */
  id: string;
  /** Tab label. */
  label: string;
  /** Tab order (Response is 0; smaller comes first). */
  order: number;
  /** Called once, the first time the tab is opened, with the view's panel element. */
  mount(el: HTMLElement, host: ViewHost): void;
  /**
   * Called when the tab is shown, and whenever a new live result lands while
   * it is shown. A view that computes something expensive should do it only
   * on request or when its inputs changed, and show what the numbers refer to.
   */
  refresh(): void;
  /** Called when the tab is hidden. */
  hide?(): void;
}

export type { EngineWorker };
