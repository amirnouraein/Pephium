import { tabs } from "$domain/tabs";

/**
 * Mouse gestures every tab's main button shares, spread onto it: a
 * middle-click closes the tab, and a double-click takes an Essential or
 * pinned tab back to the page it was kept at. Native ignores the latter for
 * any other tab, and closes a pinned tab or an Essential the way its close
 * button does, leaving it unloaded in its slot.
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
      tabs.close(id);
    },
    ondblclick: () => tabs.returnHome(id),
  };
}
