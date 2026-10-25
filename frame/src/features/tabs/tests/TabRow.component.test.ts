import { afterEach, expect, test, vi } from "vitest";
import { flushSync } from "svelte";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import { tabFixture, revision } from "$shared/testing/fixtures";
import { emitNativeEvent } from "$shared/testing/native-events";
import { events } from "$shared/ipc/native-events";
import { commands } from "$shared/ipc/bindings";
import { tabs } from "$domain/tabs";
import TabRow from "../components/TabRow.svelte";

// The row asks native directly; the spies stand in for it.
const admitted = { accepted: true, operation_id: null };
const duplicate = vi.spyOn(commands, "tabsDuplicate").mockResolvedValue(admitted);
const returnHome = vi.spyOn(commands, "tabsReturnHome").mockResolvedValue(admitted);
const rename = vi.spyOn(commands, "tabsRename").mockResolvedValue(admitted);
vi.spyOn(commands, "tabMenuPopup").mockResolvedValue(true);

afterEach(() => vi.clearAllMocks());

test("presentation sentinels expose the exact title and revision synchronously", async () => {
  const tab = tabFixture({ title: "  <img onerror=unsafe()> & title  " });
  const screen = await render(TabRow, {
    tab,
    active: true,
    splitCandidate: false,
    onSelect: vi.fn(),
    onClose: vi.fn(),
    onContextMenu: vi.fn(),
    onPointerDown: vi.fn(),
    onPointerMove: vi.fn(),
    onPointerUp: vi.fn(),
    onPointerCancel: vi.fn(),
  });
  const row = screen.container.querySelector("[data-zephium-tab-id]")!;
  expect(row.getAttribute("data-zephium-tab-id")).toBe(tab.id);
  expect(row.getAttribute("data-zephium-tab-url")).toBe(tab.url);
  expect(row.getAttribute("data-zephium-projection-revision")).toBe(tab.projection_revision);
  const label = row.querySelector("[data-zephium-tab-label]")!;
  expect(label.textContent).toBe(tab.title);
  expect(label.childElementCount).toBe(0);
  // Validate the production scoped transport, including synchronous delivery.
  const received = vi.fn();
  const stop = await events.presentationTab.listen(received);
  const next = {
    tab: { ...tab, title: "Changed", projection_revision: revision(2) },
    active: tab.id,
  };
  flushSync(() => emitNativeEvent("presentationTab", next));
  expect(received).toHaveBeenCalledExactlyOnceWith({ payload: next });
  stop();
  emitNativeEvent("presentationTab", next);
  expect(received).toHaveBeenCalledTimes(1);
});

test("close buttons show on hover by default and on every row when chosen", async () => {
  const host = document.createElement("div");
  document.body.append(host);
  await render(TabRow, {
    target: host,
    props: {
      tab: tabFixture(),
      active: false,
      splitCandidate: false,
      onSelect: vi.fn(),
      onClose: vi.fn(),
      onContextMenu: vi.fn(),
      onPointerDown: vi.fn(),
      onPointerMove: vi.fn(),
      onPointerUp: vi.fn(),
      onPointerCancel: vi.fn(),
    },
  });
  const close = host.querySelector<HTMLElement>(".tab-close")!;
  const shown = () => getComputedStyle(close).opacity;
  await expect.poll(shown).toBe("0");
  host.dataset.closeButtons = "always";
  await expect.poll(shown).toBe("1");
  host.remove();
});

function renderRow(closable: boolean) {
  const tab = tabFixture({ title: "Docs" });
  const props = {
    tab,
    active: false,
    closable,
    splitCandidate: false,
    onSelect: vi.fn(),
    onClose: vi.fn(),
    onContextMenu: vi.fn(),
    onPointerDown: vi.fn(),
    onPointerMove: vi.fn(),
    onPointerUp: vi.fn(),
    onPointerCancel: vi.fn(),
  };
  return { tab, props, screen: render(TabRow, props) };
}

test("a middle-click duplicates any row, pinned or not, and selects nothing", async () => {
  for (const closable of [true, false]) {
    duplicate.mockClear();
    const { tab, props, screen } = renderRow(closable);
    const row = (await screen).getByRole("button", { name: "Docs", exact: true });
    await userEvent.click(row, { button: "middle" });
    expect(duplicate).toHaveBeenCalledExactlyOnceWith(tab.id);
    expect(props.onClose).not.toHaveBeenCalled();
    expect(props.onSelect).not.toHaveBeenCalled();
    await userEvent.click(row, { button: "right" });
    expect(duplicate).toHaveBeenCalledTimes(1);
    (await screen).unmount();
  }
});

test("a middle press does not start autoscroll", async () => {
  const { screen } = renderRow(true);
  const row = (await screen).getByRole("button", { name: "Docs", exact: true }).element();
  const press = (button: number) => {
    const event = new MouseEvent("mousedown", { button, bubbles: true, cancelable: true });
    row.dispatchEvent(event);
    return event.defaultPrevented;
  };
  expect(press(1)).toBe(true);
  expect(press(0)).toBe(false);
});

test("a double-click asks native to take the tab back to its kept page", async () => {
  const { tab, screen } = renderRow(false);
  await userEvent.dblClick((await screen).getByRole("button", { name: "Docs", exact: true }));
  expect(returnHome).toHaveBeenCalledExactlyOnceWith(tab.id);
});

test("the tab menu's Rename edits the name in place; Escape keeps it, empty clears it", async () => {
  const { tab, screen } = renderRow(false);
  const page = await screen;
  const startRename = () => {
    tabs.openTabMenu(tab.id, 0, 0);
    tabs.renameMenuTarget();
    flushSync();
  };

  startRename();
  const field = page.getByRole("textbox", { name: "Tab name" });
  await expect.element(field).toHaveValue("Docs");
  await userEvent.keyboard("{Escape}");
  expect(rename).not.toHaveBeenCalled();
  await expect.element(page.getByRole("button", { name: "Docs", exact: true })).toBeVisible();

  startRename();
  await userEvent.fill(page.getByRole("textbox", { name: "Tab name" }), "  Mail ");
  await userEvent.keyboard("{Enter}");
  expect(rename).toHaveBeenCalledExactlyOnceWith(tab.id, "Mail");

  rename.mockClear();
  startRename();
  await userEvent.clear(page.getByRole("textbox", { name: "Tab name" }));
  await userEvent.keyboard("{Enter}");
  expect(rename).toHaveBeenCalledExactlyOnceWith(tab.id, "");
});

test("a sleeping tab fades its mark and title and says it is asleep", async () => {
  const awake = tabFixture({ title: "Docs" });
  const props = {
    tab: awake,
    active: false,
    splitCandidate: false,
    onSelect: vi.fn(),
    onClose: vi.fn(),
    onContextMenu: vi.fn(),
    onPointerDown: vi.fn(),
    onPointerMove: vi.fn(),
    onPointerUp: vi.fn(),
    onPointerCancel: vi.fn(),
  };
  const screen = await render(TabRow, props);
  const favicon = () => screen.container.querySelector("[data-favicon]");
  expect(favicon()?.hasAttribute("data-sleeping")).toBe(false);
  await expect.element(screen.getByRole("button", { name: "Docs", exact: true })).toBeVisible();

  await screen.rerender({ ...props, tab: { ...awake, availability: { state: "sleeping" } } });
  expect(favicon()?.hasAttribute("data-sleeping")).toBe(true);
  await expect
    .element(screen.getByRole("button", { name: "Docs (sleeping)", exact: true }))
    .toBeVisible();
});
