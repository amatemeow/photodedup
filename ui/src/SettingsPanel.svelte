<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError, type Counts, type IntensityId, type KnobId, type Settings, type SettingsPatch } from './lib/api';
  import { describeChange, knobDefs } from './lib/knobs';
  import { applyTheme, storedTheme, type Theme } from './lib/theme';
  import Icon from './Icon.svelte';

  // One Settings panel for every screen. What can be changed depends on where
  // it was opened from; what cannot is still shown, disabled, with the reason
  // (a tooltip and the same words on the page, which a keyboard user can read).
  //   start     nothing is running, no scan is open
  //   scanning  a scan is running
  //   review    a scan is open and its results are on screen behind the panel
  let {
    scope,
    scanId = null,
    onChanged = async () => {},
    onRescan = () => {},
    onClose,
  }: { scope: 'start' | 'scanning' | 'review'; scanId?: string | null; onChanged?: () => Promise<void>; onRescan?: () => void; onClose: () => void } = $props();

  const docked = $derived(scope === 'review'); // beside the review, which stays usable; otherwise a sheet over the screen

  let settings = $state<Settings | null>(null);
  let error = $state('');
  let saved = $state(false);
  const say = (e: unknown) => (e instanceof Error ? e.message : String(e));

  // ---- Appearance: always available.
  let theme = $state<Theme>(storedTheme());
  const themes: [Theme, string][] = [['system', 'System'], ['light', 'Light'], ['dark', 'Dark']];
  const pickTheme = (t: Theme) => {
    theme = t;
    applyTheme(t);
  };

  // ---- Scan speed: read when a scan starts, so it cannot change one that is running.
  const tiers: { id: IntensityId; title: string; blurb: string }[] = [
    {
      id: 'efficient',
      title: 'Efficient',
      blurb:
        'The quietest. Fewest workers and the gentlest scheduling, so fans stay off and the machine stays cool (about 8× less processor power than Default in testing on an Apple Silicon Mac). A scan takes much longer.',
    },
    {
      id: 'default',
      title: 'Default',
      blurb: 'Polite. A modest number of workers at lower priority, so you can keep using the computer.',
    },
    {
      id: 'performance',
      title: 'Performance',
      blurb: 'The fastest. Every core at normal priority. Expect fans and heat.',
    },
  ];
  const workers = (id: IntensityId) => settings?.tiers.find((t) => t.id === id)?.workers;
  const speedOff = $derived(scope === 'scanning' ? 'A scan is running and keeps the speed it started with. A change applies from the next scan.' : '');

  async function save(patch: SettingsPatch) {
    error = '';
    saved = false;
    try {
      settings = await api.saveSettings(patch);
      saved = true;
    } catch (e) {
      error = say(e);
    }
  }
  const choose = (id: IntensityId) => (settings && settings.scan_intensity !== id ? save({ scan_intensity: id }) : undefined);

  // ---- Matching: acts on the open scan, so it needs one.
  let unsupported = $state(false); // a scan saved before it could be regrouped
  const matchingOff = $derived(
    scope === 'scanning'
      ? 'Matching is tuned on a finished scan. Open its review once this scan is done.'
      : scope === 'start'
        ? "Matching is tuned on a scan's results. Open a scan's review to change it and watch the results move."
        : unsupported
          ? "This scan was saved by an older version and can't be regrouped in place. Scan it again once and this will work."
          : '',
  );

  let opened = $state<Record<KnobId, number> | null>(null); // the values when the review opened this
  let baseline = $state<Counts | null>(null); // what the lists held then
  let latest = $state<Counts | null>(null);
  let busy = $state(false);
  let touched = $state(false);
  let drag = $state<Partial<Record<KnobId, number>>>({}); // what a slider shows while it moves, before it is saved
  const shown = (id: KnobId) => drag[id] ?? settings?.advanced[id].value ?? 0;
  const values = (s: Settings) => Object.fromEntries(knobDefs.map((k) => [k.id, s.advanced[k.id].value])) as Record<KnobId, number>;

  onMount(async () => {
    try {
      settings = await api.settings();
      if (scope === 'review' && scanId) {
        opened = values(settings);
        // Also how we learn whether this scan can be regrouped at all.
        baseline = latest = (await api.previewRegroup(scanId, opened)).current;
      }
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) unsupported = true;
      else error = say(e);
    }
  });

  // Latest value wins: a move while an update runs is picked up by the same
  // loop afterwards, never lost and never run in parallel.
  let timer: ReturnType<typeof setTimeout> | undefined;
  let dirty = false;
  let running = false;

  function move(id: KnobId, v: number, now = false) {
    drag[id] = v;
    clearTimeout(timer);
    timer = setTimeout(go, now ? 0 : 200);
  }
  function setAll(to: Record<KnobId, number>) {
    for (const k of knobDefs) drag[k.id] = to[k.id];
    clearTimeout(timer);
    go();
  }
  function go() {
    dirty = true;
    void pump();
  }

  async function pump() {
    if (running || !scanId) return;
    running = true;
    busy = true;
    error = '';
    try {
      while (dirty) {
        dirty = false;
        settings = await api.saveSettings({ ...drag });
        latest = await api.regroup(scanId);
        touched = true;
        await onChanged();
      }
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) unsupported = true;
      else error = say(e);
    } finally {
      running = false;
      busy = false;
      // Drop only what has been saved; a slider moved meanwhile keeps its value.
      for (const k of knobDefs) if (settings && drag[k.id] === settings.advanced[k.id].value) delete drag[k.id];
    }
  }

  // The lists the moved sliders feed, before and after; several sliders can
  // feed one list, so each line is said once.
  const changes = $derived.by(() => {
    if (!baseline || !latest || !opened) return [];
    const moved = knobDefs.filter((k) => settings && settings.advanced[k.id].value !== opened![k.id]);
    return [...new Set(moved.flatMap((k) => describeChange(k.id, baseline!, latest!)))];
  });
  const atOpen = $derived(!!settings && !!opened && knobDefs.every((k) => shown(k.id) === opened![k.id]));
  const atDefault = $derived(!!settings && knobDefs.every((k) => shown(k.id) === settings!.advanced[k.id].default));
  const defaults = () => Object.fromEntries(knobDefs.map((k) => [k.id, settings!.advanced[k.id].default])) as Record<KnobId, number>;

  function onKey(e: KeyboardEvent) {
    if (!docked && e.key === 'Escape') onClose();
  }
</script>

<svelte:window onkeydown={onKey} />

{#if !docked}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={onClose}></div>
{/if}

<aside class="panel" class:docked aria-label="Settings">
  <header>
    <h2 class="lead">Settings</h2>
    <span class="state" aria-live="polite">
      {#if busy}Updating…{:else if touched}<Icon name="check" size={14} /> Review updated{:else if saved}<Icon name="check" size={14} /> Saved{/if}
    </span>
    <button class="btn icon plain" onclick={onClose} aria-label="Close settings"><Icon name="x" /></button>
  </header>

  {#if error}<div class="notice danger small" role="alert"><Icon name="info" size={15} /><span>{error}</span></div>{/if}

  {#if !settings}
    {#if !error}<div class="skeleton" style="height:220px"></div>{/if}
  {:else}
    <section>
      <h3 class="lead">Appearance</h3>
      <div class="seg" role="group" aria-label="Appearance">
        {#each themes as [key, label] (key)}
          <button aria-pressed={theme === key} onclick={() => pickTheme(key)}>{label}</button>
        {/each}
      </div>
      <p class="hint">System follows your computer's light or dark setting.</p>
    </section>

    <section class:off={!!speedOff} title={speedOff}>
      <h3 class="lead">Scan speed</h3>
      <p class="hint">How hard a scan may push this computer, for every scan from now on. This machine has {settings.logical_cores} cores.</p>
      {#if speedOff}<p class="why" id="why-speed"><Icon name="info" size={14} /><span>{speedOff}</span></p>{/if}
      <fieldset class="tiers" disabled={!!speedOff} aria-label="Scan speed" aria-describedby={speedOff ? 'why-speed' : undefined}>
        {#each tiers as t (t.id)}
          <label class="tier" class:on={settings.scan_intensity === t.id}>
            <input type="radio" name="intensity" value={t.id} checked={settings.scan_intensity === t.id} onchange={() => choose(t.id)} />
            <span class="body">
              <span class="head">
                <strong>{t.title}</strong>
                {#if t.id === 'default'}<span class="badge accent">Recommended</span>{/if}
                <span class="hint">{workers(t.id)} worker{workers(t.id) === 1 ? '' : 's'}</span>
              </span>
              <span class="secondary">{t.blurb}</span>
            </span>
          </label>
        {/each}
      </fieldset>
    </section>

    <section class="matching" class:off={!!matchingOff} title={matchingOff}>
      <h3 class="lead">Matching</h3>
      <p class="hint">How strictly photos are grouped. Moving a slider regroups the open scan and the review changes as you go; your choices stay with the photos.</p>
      {#if matchingOff}
        <p class="why" id="why-matching">
          <Icon name="info" size={14} />
          <span>
            {matchingOff}
            {#if scope === 'review' && unsupported}<button class="link" onclick={onRescan}>Scan again</button>{/if}
          </span>
        </p>
      {/if}
      <fieldset class="knobs" disabled={!!matchingOff} aria-describedby={matchingOff ? 'why-matching' : undefined}>
        {#each knobDefs as k (k.id)}
          {@const kn = settings.advanced[k.id]}
          <div class="knob">
            <div class="knob-head">
              <label for="set-{k.id}"><strong>{k.title}</strong></label>
              <span class="value">{shown(k.id)}{k.unit}</span>
            </div>
            <div class="slider">
              <span class="hint">{k.low}</span>
              <input
                id="set-{k.id}"
                type="range"
                min={kn.min}
                max={kn.max}
                step={k.step}
                value={shown(k.id)}
                style="--p: {((shown(k.id) - kn.min) / (kn.max - kn.min)) * 100}%"
                oninput={(e) => move(k.id, Number(e.currentTarget.value))}
                onchange={(e) => move(k.id, Number(e.currentTarget.value), true)}
              />
              <span class="hint">{k.high}</span>
            </div>
            <p class="hint">{k.blurb}</p>
            {#if k.caution}<p class="hint caution">{k.caution}</p>{/if}
          </div>
        {/each}
        {#if !matchingOff}
          <div class="changes" aria-live="polite">
            {#if changes.length}
              <strong>Since you opened this</strong>
              <ul>{#each changes as l (l)}<li>{l}</li>{/each}</ul>
            {:else}
              <span class="hint">No change to the review yet.</span>
            {/if}
          </div>
        {/if}
        <div class="actions">
          <button class="btn" disabled={!!matchingOff || atOpen} onclick={() => setAll(opened!)} title="Put the sliders back to where they were when you opened this">Revert</button>
          <button class="btn" disabled={!!matchingOff || atDefault} onclick={() => setAll(defaults())}>Defaults</button>
        </div>
      </fieldset>
    </section>

    <p class="hint foot">“Include videos” is on the start screen, and the sort order is on the review screen.</p>
  {/if}
</aside>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 30;
    background: var(--scrim);
    animation: fade var(--t) var(--ease);
  }
  .panel {
    display: flex;
    flex-direction: column;
    gap: var(--s4);
    padding: var(--s4);
    overflow-y: auto;
    background: var(--surface);
    border-left: 1px solid var(--separator);
    min-height: 0;
  }
  /* as a sheet over the start and scanning screens */
  .panel:not(.docked) {
    position: fixed;
    z-index: 31;
    top: 0;
    right: 0;
    bottom: 0;
    width: min(400px, 100%);
    box-shadow: var(--shadow-lg);
    animation: slide var(--t) var(--ease);
  }
  @keyframes slide {
    from {
      transform: translateX(24px);
      opacity: 0;
    }
  }
  /* narrow windows: beside the review there is no room, so a sheet over its bottom */
  @media (max-width: 860px) {
    .panel.docked {
      position: fixed;
      left: 0;
      right: 0;
      bottom: 0;
      max-height: 55vh;
      z-index: 5;
      border-left: 0;
      border-top: 1px solid var(--separator);
      box-shadow: 0 -8px 24px rgb(0 0 0 / 0.12);
    }
  }
  header {
    display: flex;
    align-items: center;
    gap: var(--s2);
    order: -2;
  }
  header h2 {
    margin: 0;
  }
  .state {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: var(--s1);
    color: var(--success);
    font-size: var(--fs-small);
    font-weight: 600;
  }
  section {
    display: grid;
    gap: var(--s2);
  }
  /* beside a review, matching is the reason to open this, so it comes first */
  .panel.docked .matching {
    order: -1;
  }
  section h3 {
    margin: 0;
  }
  section p {
    margin: 0;
  }
  /* unavailable here: still shown, dimmed, with the reason */
  section.off > :not(h3):not(.why):not(.hint) {
    opacity: 0.5;
  }
  fieldset {
    border: 0;
    padding: 0;
    margin: 0;
    min-width: 0;
  }
  .why {
    display: flex;
    gap: var(--s2);
    align-items: flex-start;
    padding: var(--s2) var(--s3);
    border-radius: var(--radius-sm, 8px);
    background: color-mix(in srgb, var(--accent) 9%, var(--surface));
    font-size: var(--fs-small);
  }
  .why :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .tiers {
    display: grid;
    gap: var(--s2);
  }
  .tier {
    display: flex;
    gap: var(--s3);
    align-items: flex-start;
    padding: var(--s3);
    border-radius: var(--radius);
    background: var(--surface);
    box-shadow: var(--shadow-sm);
    cursor: pointer;
    transition: box-shadow var(--t-fast) var(--ease);
  }
  fieldset:disabled .tier {
    cursor: not-allowed;
  }
  .tier:hover {
    box-shadow: 0 0 0 1px var(--separator), 0 3px 10px rgb(0 0 0 / 0.08);
  }
  .tier.on {
    box-shadow: 0 0 0 2px var(--accent);
    background: color-mix(in srgb, var(--accent) 6%, var(--surface));
  }
  .tier input {
    appearance: none;
    flex: none;
    width: 18px;
    height: 18px;
    margin: 2px 0 0;
    border-radius: 50%;
    background: var(--field);
    box-shadow: inset 0 0 0 1.5px var(--label-3);
    cursor: inherit;
  }
  .tier input:checked {
    background: radial-gradient(circle, var(--accent-fg) 0 28%, var(--accent) 32%);
    box-shadow: none;
  }
  .tier input:focus-visible {
    box-shadow: var(--ring);
  }
  .body {
    display: grid;
    gap: var(--s1);
  }
  .head {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--s2);
  }
  .knobs {
    display: grid;
    gap: var(--s3);
  }
  .knob {
    display: grid;
    gap: var(--s1);
    padding-top: var(--s3);
    border-top: 1px solid var(--separator);
  }
  .knob-head {
    display: flex;
    align-items: baseline;
    gap: var(--s3);
    justify-content: space-between;
  }
  .value {
    color: var(--accent);
    font-weight: 650;
    font-variant-numeric: tabular-nums;
  }
  .slider {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: var(--s2);
  }
  .changes {
    display: grid;
    gap: var(--s1);
    padding-top: var(--s3);
    border-top: 1px solid var(--separator);
    font-size: var(--fs-small);
    font-variant-numeric: tabular-nums;
  }
  .changes ul {
    margin: 0;
    padding-left: var(--s4);
  }
  .actions {
    display: flex;
    gap: var(--s2);
  }
  .foot {
    margin: 0;
  }
  .small {
    font-size: var(--fs-small);
  }
</style>
