import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import { emitNativeEvent } from "$shared/testing/native-events";
import { revision, tabFixture } from "$shared/testing/fixtures";
import { tabs } from "$domain/tabs";
import SidebarHeader from "../components/SidebarHeader.svelte";

const native = vi.hoisted(() => ({
  reload: vi.fn(async () => ({ accepted: true, operation_id: null })),
  duplicate: vi.fn(async () => ({ accepted: true, operation_id: null })),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsBootstrap: async () => {},
    tabsReload: native.reload,
    tabsDuplicate: native.duplicate,
  });
});

afterEach(() => {
  tabs.dispose();
  vi.clearAllMocks();
});

test("Reload reloads the active tab on a click and duplicates it on a middle-click", async () => {
  await tabs.init();
  const open = tabFixture({ id: "open", title: "Example" });
  emitNativeEvent("itemsChanged", {
    projection_revision: revision(20),
    profile: null,
    spaces: [],
    active_space_id: null,
    nodes: [],
    tabs: [open],
    active: open.id,
    split_group: null,
  });
  const screen = await render(SidebarHeader, { compact: false, ontoggle: vi.fn() });
  const reload = screen.getByRole("button", { name: "Reload", exact: true });

  await userEvent.click(reload, { button: "middle" });
  expect(native.duplicate).toHaveBeenCalledExactlyOnceWith(open.id);
  expect(native.reload).not.toHaveBeenCalled();

  await reload.click();
  expect(native.reload).toHaveBeenCalledExactlyOnceWith(open.id);
  expect(native.duplicate).toHaveBeenCalledTimes(1);

  const press = new MouseEvent("mousedown", { button: 1, bubbles: true, cancelable: true });
  reload.element().dispatchEvent(press);
  expect(press.defaultPrevented).toBe(true);
});
