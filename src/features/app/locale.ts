/** Keeps the UI locale in sync with `appearance.locale`; re-registers localised contributions. */
import { registerActions, type Action } from "@/platform/actions";
import { setLocale, useI18n } from "@/platform/i18n";
import { commands, isTauri, type Settings } from "@/platform/ipc";
import { useProject } from "@/platform/project";

export function localeSetting(settings: Settings | null | undefined): string | undefined {
  return settings?.appearance?.locale;
}

/** Settings read before the first render; reused by `installLocaleSync` (no second IPC). */
let primed: Settings | null = null;
let primedValue: string | undefined;

/**
 * Applies the stored language (default: follow the OS) before React renders, so a
 * Chinese system does not flash English first. The workbench renders before
 * `useProject.settings` is filled, hence the read here.
 */
export async function primeLocale(): Promise<void> {
  if (isTauri()) {
    try {
      primed = await commands.settingsGet();
    } catch {
      // Unreadable settings: follow the system language.
    }
  }
  primedValue = localeSetting(primed);
  await setLocale(primedValue ?? "system");
}

export function installLocaleSync(): () => void {
  let current = primedValue;
  const apply = (settings: Settings | null | undefined) => {
    const value = localeSetting(settings);
    if (value === undefined || value === current) return;
    current = value;
    void setLocale(value);
  };
  const initial = useProject.getState().settings ?? primed;
  if (initial) apply(initial);
  else if (isTauri()) void commands.settingsGet().then(apply, () => {});
  return useProject.subscribe((s, prev) => {
    if (s.settings !== prev.settings) apply(s.settings);
  });
}

/**
 * Registers actions whose titles come from `t()`, re-registering them when the
 * locale changes. Other features can copy this pattern.
 */
export function registerLocalizedActions(factory: () => Action[]): () => void {
  let dispose = registerActions(factory());
  const unsub = useI18n.subscribe((s, prev) => {
    if (s.messages === prev.messages) return;
    dispose();
    dispose = registerActions(factory());
  });
  return () => {
    unsub();
    dispose();
  };
}
