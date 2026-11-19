const onBackground = ({ target, currentTarget }: MouseEvent) =>
  target === currentTarget ||
  (target instanceof HTMLElement && target.hasAttribute("data-tab-list-background"));

/** A middle press that begins and ends on empty list space, outside its rows. */
export function openOnBackgroundMiddleClick(open: () => void) {
  let pressed = false;
  let released = false;
  return {
    onmousedown(event: MouseEvent) {
      if (event.button !== 1) return;
      pressed = onBackground(event);
      released = false;
      if (pressed) event.preventDefault();
    },
    onmouseup(event: MouseEvent) {
      if (event.button === 1) released = onBackground(event);
    },
    onauxclick(event: MouseEvent) {
      if (event.button !== 1) return;
      // auxclick can target the common ancestor even if the release hit a row.
      const hit = pressed && released && onBackground(event);
      pressed = released = false;
      if (!hit) return;
      event.preventDefault();
      open();
    },
  };
}
