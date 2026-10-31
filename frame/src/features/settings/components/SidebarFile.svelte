<script lang="ts">
  import * as m from "$shared/i18n/messages";
  import {
    commands,
    type SidebarFileProblemView,
    type SidebarFileReplyView,
  } from "$shared/ipc/bindings";
  import SettingsRow from "$shared/ui/SettingsRow";
  import Button from "$shared/ui/Button";

  // Native picks, reads and writes the file; chrome only asks and reports.
  let busy = $state<"export" | "import" | null>(null);
  let result = $state<{ text: string; failed: boolean } | null>(null);

  const problems: Record<SidebarFileProblemView, () => string> = {
    too_large: m.sidebar_file_too_large,
    not_sidebar_file: m.sidebar_file_not_export,
    newer_version: m.sidebar_file_newer,
    too_many: m.sidebar_file_too_many,
    invalid: m.sidebar_file_invalid,
    unreadable: m.sidebar_file_unreadable,
    unavailable: m.sidebar_file_unavailable,
  };

  function describe(kind: "export" | "import", reply: SidebarFileReplyView) {
    switch (reply.outcome) {
      case "cancelled":
        return null;
      case "failed":
        return { text: problems[reply.problem](), failed: true };
      case "done":
        if (kind === "export") return { text: m.sidebar_file_saved(), failed: false };
        if (reply.tabs === 0) return { text: m.sidebar_file_current(), failed: false };
        return {
          text:
            reply.tabs === 1
              ? m.sidebar_file_added_one()
              : m.sidebar_file_added({ count: new Intl.NumberFormat().format(reply.tabs) }),
          failed: false,
        };
    }
  }

  async function run(kind: "export" | "import") {
    if (busy) return;
    busy = kind;
    result = null;
    let reply: SidebarFileReplyView;
    try {
      reply = await (kind === "export" ? commands.sidebarExport() : commands.sidebarImport());
    } catch {
      reply = { outcome: "failed", problem: "unavailable" };
    } finally {
      busy = null;
    }
    result = describe(kind, reply);
  }
</script>

<div data-setting="general.sidebar">
  <SettingsRow title={m.sidebar_file_title()} description={m.sidebar_file_help()}>
    <div class="actions">
      <Button
        size="compact"
        variant="ghost"
        pending={busy === "export"}
        disabled={busy === "import"}
        onclick={() => void run("export")}>{m.sidebar_file_export()}</Button
      >
      <Button
        size="compact"
        pending={busy === "import"}
        disabled={busy === "export"}
        onclick={() => void run("import")}>{m.sidebar_file_import()}</Button
      >
    </div>
  </SettingsRow>
  {#if result}<p
      class="result"
      class:error={result.failed}
      role={result.failed ? "alert" : "status"}
    >
      {result.text}
    </p>{/if}
</div>

<style>
  .actions {
    display: flex;
    gap: 8px;
  }

  .result {
    margin: 0 18px 14px;
    color: var(--color-muted);
    font-size: var(--text-label);
  }

  .error {
    color: var(--color-danger);
  }
</style>
