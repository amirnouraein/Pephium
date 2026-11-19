<script lang="ts">
  import { DropdownMenu } from "bits-ui";
  import * as m from "$shared/i18n/messages";
  import { tabs } from "$domain/tabs";
  import { favicons } from "$domain/favicons";
  import FavIcon from "$shared/ui/FavIcon";
  import Icon from "$shared/ui/Icon";
  import { layoutAnchor } from "$shared/ui/Menu/anchor";
  import { menuMaterialFor } from "$shared/ui/Menu/material";
  import type { AssistSession } from "../lib/assist-session.svelte";
  import {
    Add01Icon,
    ArrowLeft01Icon,
    ArrowRight01Icon,
    BrowserIcon,
    Globe02Icon,
    Target02Icon,
    Tick02Icon,
  } from "../lib/icons";
  import "$shared/ui/Menu/popover.css";

  let { session, onpick }: { session: AssistSession; onpick: () => void } = $props();

  let open = $state(false);
  let view = $state<"root" | "tabs">("root");
  let trigger = $state<HTMLButtonElement | null>(null);
  let material = $state<"opaque" | undefined>();
  let content = $state<HTMLElement | null>(null);
  // Stepping in lands on the first tab; stepping back, on the row that stepped in.
  function step(next: "root" | "tabs") {
    view = next;
    const root = content;
    requestAnimationFrame(() =>
      root?.querySelector<HTMLElement>(next === "tabs" ? ".tab" : ".into")?.focus(),
    );
  }

  const web = (url: string | null) => !!url && /^https?:\/\//u.test(url);
  const hostOf = (url: string) => {
    try {
      return new URL(url).host.replace(/^www\./u, "");
    } catch {
      return url;
    }
  };
  const others = $derived(
    tabs
      .tabs()
      .filter((tab) => tab.id !== session.tab && web(tab.url))
      .slice(0, 24),
  );
  const canPick = $derived(!!session.status?.can_pick && session.picking === null);
</script>

<DropdownMenu.Root
  bind:open
  onOpenChange={(next) => {
    if (next) material = menuMaterialFor(trigger);
    else view = "root";
  }}
>
  <DropdownMenu.Trigger bind:ref={trigger} class="tool" aria-label={m.assist_add()}>
    <Icon icon={Add01Icon} size={16} strokeWidth={1.8} />
  </DropdownMenu.Trigger>
  <DropdownMenu.Portal>
    <DropdownMenu.Content
      bind:ref={content}
      data-menu-material={material}
      class="ui-menu ui-menu-scroll assist-add"
      side="top"
      align="start"
      customAnchor={layoutAnchor(trigger)}
      sideOffset={8}
      collisionPadding={10}
    >
      {#if view === "root"}
        <DropdownMenu.Item
          class="ui-menu-item"
          disabled={!session.attachable || session.pageAttached}
          onSelect={() => (session.pageOff = false)}
        >
          <span class="ui-menu-icon" aria-hidden="true"><Icon icon={Globe02Icon} size={15} /></span
          >{m.assist_attach_page()}
        </DropdownMenu.Item>
        <DropdownMenu.Item class="ui-menu-item" disabled={!canPick} onSelect={onpick}>
          <span class="ui-menu-icon" aria-hidden="true"><Icon icon={Target02Icon} size={15} /></span
          >{m.assist_pick()}
        </DropdownMenu.Item>
        <DropdownMenu.Separator class="ui-menu-separator" />
        <DropdownMenu.Item
          class="ui-menu-item into"
          disabled={others.length === 0}
          onSelect={(event) => {
            event.preventDefault();
            step("tabs");
          }}
        >
          <span class="ui-menu-icon" aria-hidden="true"><Icon icon={BrowserIcon} size={15} /></span
          >{m.assist_attach_tab()}<span class="more" aria-hidden="true"
            ><Icon icon={ArrowRight01Icon} size={13} strokeWidth={1.8} /></span
          >
        </DropdownMenu.Item>
      {:else}
        <DropdownMenu.Item
          class="ui-menu-item back"
          onSelect={(event) => {
            event.preventDefault();
            step("root");
          }}
        >
          <span class="ui-menu-icon" aria-hidden="true"
            ><Icon icon={ArrowLeft01Icon} size={14} strokeWidth={1.8} /></span
          >{m.assist_attach_tab()}
        </DropdownMenu.Item>
        <DropdownMenu.Separator class="ui-menu-separator" />
        {#each others as tab (tab.id)}
          {@const mark = favicons.mark(tab.icon, tab.url)}
          {@const attached = session.others.includes(tab.id)}
          <DropdownMenu.Item
            class="ui-menu-item tab"
            textValue={tab.title || tab.url || ""}
            onSelect={(event) => {
              // Stays open so several tabs can be attached in one visit.
              event.preventDefault();
              if (attached) session.detachTab(tab.id);
              else session.attachTab(tab.id);
            }}
          >
            <span class="ui-menu-icon" aria-hidden="true"
              ><FavIcon
                image={mark?.image ?? null}
                tone={mark?.tone}
                size={15}
                fallback={Globe02Icon}
              /></span
            >
            <span class="words">
              <span class="title">{tab.title || hostOf(tab.url ?? "")}</span>
              <span class="host">{hostOf(tab.url ?? "")}</span>
            </span>
            {#if attached}<span class="tick" aria-hidden="true"
                ><Icon icon={Tick02Icon} size={14} strokeWidth={2} /></span
              >{/if}
          </DropdownMenu.Item>
        {/each}
      {/if}
    </DropdownMenu.Content>
  </DropdownMenu.Portal>
</DropdownMenu.Root>

<style>
  :global(.ui-menu.assist-add) {
    inline-size: min(272px, calc(100vw - 24px));
    max-block-size: min(var(--bits-floating-available-height, 420px), 380px);
  }

  .more,
  .tick {
    display: grid;
    flex: none;
    place-items: center;
    margin-inline-start: auto;
    padding-inline-start: 12px;
    color: var(--color-faint);
  }

  .tick {
    color: var(--color-text);
  }

  :global(.assist-add .back) {
    color: var(--color-muted);
    font-weight: 500;
  }

  :global(.assist-add .tab) {
    padding-block: 7px;
  }

  .words {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-inline-size: 0;
  }

  .title,
  .host {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .host {
    color: var(--color-faint);
    font-size: var(--text-caption);
    line-height: 14px;
  }
</style>
