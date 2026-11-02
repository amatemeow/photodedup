<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError, type Counts, type KnobId, type Settings } from './lib/api';
  import { describeChange, knobDefs } from './lib/knobs';
  import Icon from './Icon.svelte';
  import Pending from './Pending.svelte';
  import { known, refreshSettings } from './lib/settings';

  // The matching sliders, beside the review they act on. Moving one saves it,
  // groups the scan again and has the review reload, so the lists change while
  // you watch. Nothing is lost by trying: your choices belong to files, not to
  // groups, and Revert puts back what the sliders were when this opened.
  let {
    onChanged,
    onRescan,
    onClose,
    scanId,
  }: { scanId: string; onChanged: () => Promise<void>; onRescan: () => void; onClose: () => void } = $props();

  let settings = $state<Settings | null>(known.value);
  let opened = $state<Record<KnobId, number> | null>(null); // the values when this opened
  let baseline = $state<Counts | null>(null); // what the lists held then
  let latest = $state<Counts | null>(null);
  let unsupported = $state(false); // a scan saved before this could be done
  let busy = $state(false);
  let touched = $state(false);
  let error = $state('');
  // What a slider shows while it is moving, before it has been saved.
  let drag = $state<Partial<Record<KnobId, number>>>({});

  const say = (e: unknown) => (e instanceof Error ? e.message : String(e));
  const shown = (id: KnobId) => drag[id] ?? settings?.advanced[id].value ?? 0;
  const values = (s: Settings) => Object.fromEntries(knobDefs.map((k) => [k.id, s.advanced[k.id].value])) as Record<KnobId, number>;

  onMount(async () => {
    try {
      settings = await refreshSettings();
      opened = values(settings);
      // Also how we learn whether this scan can be tuned at all.
      baseline = latest = (await api.previewRegroup(scanId, opened)).current;
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) unsupported = true;
      else error = say(e);
    }
  });

  // Latest value wins: a move while an update is running is picked up by the
  // same loop afterwards, never lost and never run in parallel.
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
    if (running) return;
    running = true;
    busy = true;
    error = '';
    try {
      while (dirty) {
        dirty = false;
        const patch = { ...drag };
        settings = await api.saveSettings(patch);
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

  // The lists the moved sliders feed, before and after; several sliders can feed
  // one list, so each line is said once.
  const changes = $derived.by(() => {
    if (!baseline || !latest || !opened) return [];
    const moved = knobDefs.filter((k) => settings && settings.advanced[k.id].value !== opened![k.id]);
    return [...new Set(moved.flatMap((k) => describeChange(k.id, baseline!, latest!)))];
  });
  const atOpen = $derived(!!settings && !!opened && knobDefs.every((k) => shown(k.id) === opened![k.id]));
  const atDefault = $derived(!!settings && knobDefs.every((k) => shown(k.id) === settings!.advanced[k.id].default));
  const defaults = () => Object.fromEntries(knobDefs.map((k) => [k.id, settings!.advanced[k.id].default])) as Record<KnobId, number>;
</script>

<aside class="tuning" aria-label="Fine-tune matching">
  <header>
    <h2 class="lead">Fine-tune</h2>
    <span class="state" aria-live="polite">{#if busy}Updating…{:else if touched}<Icon name="check" size={14} /> Review updated{/if}</span>
    <button class="btn icon plain" onclick={onClose} aria-label="Close fine-tuning"><Icon name="x" /></button>
  </header>

  {#if error}<div class="notice danger small" role="alert"><Icon name="info" size={15} /><span>{error}</span></div>{/if}

  {#if unsupported}
    <div class="notice accent small" role="status">
      <Icon name="info" size={15} />
      <span>
        This scan was saved before the sliders could regroup it. Scan the folder again once and they will work here.
        <button class="link" onclick={onRescan}>Scan again</button>
      </span>
    </div>
  {:else if !settings || !baseline}
    {#if !error}<Pending what="the matching settings" height={180} />{/if}
  {:else}
    <p class="hint intro">Drag a slider and the review changes as you go. Your keep and quarantine choices stay with the photos.</p>
    {#each knobDefs as k (k.id)}
      {@const kn = settings.advanced[k.id]}
      <div class="knob">
        <div class="knob-head">
          <label for="tune-{k.id}"><strong>{k.title}</strong></label>
          <span class="value">{shown(k.id)}{k.unit}</span>
        </div>
        <div class="slider">
          <span class="hint">{k.low}</span>
          <input
            id="tune-{k.id}"
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
    <div class="changes" aria-live="polite">
      {#if changes.length}
        <strong>Since you opened this</strong>
        <ul>{#each changes as l (l)}<li>{l}</li>{/each}</ul>
      {:else}
        <span class="hint">No change to the review yet.</span>
      {/if}
    </div>
    <div class="actions">
      <button class="btn" disabled={atOpen} onclick={() => setAll(opened!)} title="Put the sliders back to where they were when you opened this">Revert</button>
      <button class="btn" disabled={atDefault} onclick={() => setAll(defaults())}>Defaults</button>
    </div>
  {/if}
</aside>

<style>
  .tuning {
    display: flex;
    flex-direction: column;
    gap: var(--s3);
    padding: var(--s4);
    overflow-y: auto;
    background: var(--surface);
    border-left: 1px solid var(--separator);
    min-height: 0;
  }
  header {
    display: flex;
    align-items: center;
    gap: var(--s2);
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
  .intro {
    margin: 0;
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
  .caution {
    color: var(--warning-fg, var(--label-2));
  }
  .actions {
    display: flex;
    gap: var(--s2);
  }
  .small {
    font-size: var(--fs-small);
  }
  /* narrow windows: a sheet over the bottom of the review, which stays visible above it */
  @media (max-width: 860px) {
    .tuning {
      position: fixed;
      left: 0;
      right: 0;
      bottom: 0;
      max-height: 48vh;
      z-index: 5;
      border-left: 0;
      border-top: 1px solid var(--separator);
      box-shadow: 0 -8px 24px rgb(0 0 0 / 0.12);
    }
  }
</style>
