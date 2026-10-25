import { tabs } from "$domain/tabs";

/**
 * Mouse gestures every tab's main button shares, spread onto it: a
 * middle-click duplicates the tab, and a double-click takes an Essential or
 * pinned tab back to the page it was kept at. Native ignores the latter for
 * any other tab.
 */
export function tabGestures(id: string) {
  return {
    // A middle press starts autoscroll in Chromium (WebView2); WebKit has none.
    onmousedown: (event: MouseEvent) => {
      if (event.button === 1) event.preventDefault();
    },
    onauxclick: (event: MouseEvent) => {
      if (event.button !== 1) return;
      event.preventDefault();
      tabs.duplicate(id);
    },
    ondblclick: () => tabs.returnHome(id),
  };
}
