/** Thin wrapper around window.location for testability (jsdom 26 freezes location methods) */
export function navigateTo(url: string): void {
  window.location.assign(url)
}
