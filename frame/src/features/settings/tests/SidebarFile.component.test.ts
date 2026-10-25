import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import type { SidebarFileReplyView } from "$shared/ipc/bindings";
import SidebarFile from "../components/SidebarFile.svelte";

const native = vi.hoisted(() => ({
  exported: vi.fn(async (): Promise<SidebarFileReplyView> => ({ outcome: "done", tabs: 9 })),
  imported: vi.fn(async (): Promise<SidebarFileReplyView> => ({ outcome: "cancelled" })),
}));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    sidebarExport: native.exported as never,
    sidebarImport: native.imported as never,
  });
});

test("export and import report what happened, and a refused file says why", async () => {
  const screen = await render(SidebarFile);
  await screen.getByRole("button", { name: "Export…" }).click();
  await expect.element(screen.getByRole("status")).toHaveTextContent("Saved to a file");

  await screen.getByRole("button", { name: "Import…" }).click();
  expect(native.imported).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("status").query()).toBeNull();

  native.imported.mockResolvedValueOnce({ outcome: "failed", problem: "newer_version" });
  await screen.getByRole("button", { name: "Import…" }).click();
  await expect.element(screen.getByRole("alert")).toHaveTextContent(/newer version of Zephium/u);

  native.imported.mockResolvedValueOnce({ outcome: "done", tabs: 3 });
  await screen.getByRole("button", { name: "Import…" }).click();
  await expect.element(screen.getByRole("status")).toHaveTextContent("Added 3 tabs");

  native.imported.mockResolvedValueOnce({ outcome: "done", tabs: 0 });
  await screen.getByRole("button", { name: "Import…" }).click();
  await expect
    .element(screen.getByRole("status"))
    .toHaveTextContent("Everything in that file is already here");
});
