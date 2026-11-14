import * as m from "$shared/i18n/messages";
import type { TabView } from "$shared/ipc/bindings";
import { tabs } from "$domain/tabs";

/**
 * What a tab is called to the person, saying when it is asleep. Kept out of
 * the tabs domain, which lazy pages import too, so its strings load with the
 * sidebar alone.
 */
export function tabLabel(tab: TabView) {
  const title = tab.title || m.untitled_tab();
  return tabs.asleep(tab) ? m.tab_sleeping_named({ title }) : title;
}
