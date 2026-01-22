export {};

declare global {
  /**
   * If blocks guarded by __ASSERT__ will be folded away in builds.
   */
  const __ASSERT__: boolean;

  /**
   * True when running tests, false otherwise.
   */
  const __TEST__: boolean;
}
