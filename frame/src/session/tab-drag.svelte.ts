import { commands } from "$shared/ipc/bindings";
import { settle } from "$domain/operations";
import { tabs } from "$domain/tabs";
let id = $state<string | null>(null);
let over = $state(false);
let failed = $state(false);
let pending = false;
let gesture = 0;
/** Where a dropped tab goes: the kept sites, the space's pinned tabs, or its open ones. */
export type Destination = "essentials" | "pinned" | "today";
export const draggedId = () => id;
export const overEssentials = () => over;
export const moveFailed = () => failed;
export function begin(value: string) {
  gesture++;
  id = value;
  failed = false;
}
/** Only a web page can be kept: a tab with nothing open yet, or one the
 *  browser itself owns, has no site to keep. */
export function keepable(value: string) {
  const tab = tabs.tabs().find((candidate) => candidate.id === value);
  return !!tab?.url && (tab.content ?? "web") === "web";
}
export function hover(x: number, y: number) {
  over =
    id !== null &&
    keepable(id) &&
    !!document.elementFromPoint(x, y)?.closest("[data-essentials-drop]");
}
export function end() {
  const active = id !== null;
  const generation = ++gesture;
  id = null;
  over = false;
  if (active)
    void commands.tabDragOver(null, null).catch(() => {
      if (generation === gesture) failed = true;
    });
}
/** Takes a tab out of the split it is in, then places it as `place` would. */
export async function leaveSplit(
  value: string,
  target: { to: Destination; before: string | null } | null,
) {
  if (pending) return;
  pending = true;
  failed = false;
  try {
    const result = await settle(commands.tabsLeaveSplit(value), 5000);
    if (result.outcome === "failed" || result.outcome === "rejected") throw new Error("settlement");
  } catch {
    failed = true;
    return;
  } finally {
    pending = false;
  }
  if (target) await place(value, target.to, target.before);
}

export function move(value: string, essential: boolean, before: string | null = null) {
  return place(value, essential ? "essentials" : "today", before);
}

export async function place(value: string, to: Destination, before: string | null = null) {
  if (pending) return;
  pending = true;
  failed = false;
  try {
    const admission =
      to === "pinned"
        ? commands.tabsSetPinned(value, true, before)
        : commands.tabsSetEssential(value, to === "essentials", before);
    const result = await settle(admission, 5000);
    if (result.outcome === "failed" || result.outcome === "rejected") throw new Error("settlement");
  } catch {
    failed = true;
  } finally {
    pending = false;
  }
}
