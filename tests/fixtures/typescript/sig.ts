// A parameter list that the def_sig depth scan used to mis-read (#43).
//
// Its own file, and one SCIP has no document for, deliberately: every file in
// `index.scip` carries a module symbol whose enclosing range stops at the last
// line scip saw, so growing one of them re-attributes its module-scope
// references from the file to that module symbol and moves rows in
// `calls` — a separate question from this one (#75).

/** `=>` is an operator, not a closing bracket. */
export function listen(on: (key: string) => void, options: { once: boolean }): void {
  void on;
  void options;
}

/** `<=` and `>=` are operators too, and a real generic is still a bracket. */
export function clamp<T extends number = number>(value: T, high: T): T {
  return value <= high ? value : high;
}
