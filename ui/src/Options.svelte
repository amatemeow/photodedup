<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError, type IntensityId, type KnobId, type Settings, type SettingsPatch } from './lib/api';

  let { onBack }: { onBack: () => void } = $props();

  let settings = $state<Settings | null>(null);
  let error = $state('');
  let saved = $state(false);

  // What each choice really does, in the order from quietest to fastest.
  // The numbers come from measurements on the development machine, so they
  // are worded as such rather than promised.
  const tiers: { id: IntensityId; title: string; blurb: string }[] = [
    {
      id: 'efficient',
      title: 'Efficient',
      blurb:
        'The quietest. Fewest workers and the gentlest scheduling, so fans stay off and the machine stays cool (about 8× less processor power than Default in testing on an Apple Silicon Mac). A scan takes much longer: choose it when you can leave it running.',
    },
    {
      id: 'default',
      title: 'Default',
      blurb:
        'Polite. A modest number of workers at lower priority, so you can keep using the computer. On a Mac with efficiency cores it aims at those, but may use the fast ones when it needs to.',
    },
    {
      id: 'performance',
      title: 'Performance',
      blurb: 'The fastest. Every core at normal priority. Expect fans and heat; best when you are not using the computer for anything else.',
    },
  ];

  // The three matching thresholds, as sliders. `low`/`high` label the two
  // ends; `unit` is shown after the value; `step` keeps the slider easy to
  // land on a sensible number (the limits themselves come from the server).
  const knobs: { id: KnobId; title: string; blurb: string; low: string; high: string; unit: string; step: number; caution?: string }[] = [
    {
      id: 'sequence_similarity',
      title: 'Sequence similarity',
      blurb:
        'How alike shots must look to stay in one quick sequence. Shots that differ by more than this are split into separate groups, so a lower value gives smaller, tighter sequences.',
      low: 'Stricter',
      high: 'Looser',
      unit: '',
      step: 5,
    },
    {
      id: 'sequence_gap_seconds',
      title: 'Sequence time gap',
      blurb:
        'Shots from one camera taken within this many seconds of each other can belong to the same quick sequence. It also decides whether two similar shots are listed as a sequence or under Same-device review.',
      low: 'Shorter',
      high: 'Longer',
      unit: ' s',
      step: 1,
    },
    {
      id: 'match_distance',
      title: 'Copy match distance',
      blurb: 'How different two photos may look and still be checked as possible copies of each other.',
      low: 'Stricter',
      high: 'More permissive',
      unit: '',
      step: 5,
      caution:
        'A higher value finds more copies but can also suggest wrong ones. Two untouched camera photos are never suggested as copies of each other, whatever you choose here.',
    },
  ];

  // What a slider shows while it is being dragged, before it is saved.
  let drag = $state<Partial<Record<KnobId, number>>>({});
  const shown = (id: KnobId) => drag[id] ?? settings?.advanced[id].value ?? 0;

  const say = (e: unknown) => (e instanceof ApiError ? e.message : e instanceof Error ? e.message : String(e));

  onMount(async () => {
    try {
      settings = await api.settings();
    } catch (e) {
      error = say(e);
    }
  });

  async function save(patch: SettingsPatch) {
    error = '';
    saved = false;
    try {
      settings = await api.saveSettings(patch);
      saved = true;
    } catch (e) {
      error = say(e);
    } finally {
      drag = {};
    }
  }

  const choose = (id: IntensityId) => (settings && settings.scan_intensity !== id ? save({ scan_intensity: id }) : undefined);

  const allDefault = $derived(settings ? knobs.every((k) => settings!.advanced[k.id].value === settings!.advanced[k.id].default) : true);
  const resetAll = () =>
    settings &&
    save(Object.fromEntries(knobs.map((k) => [k.id, settings!.advanced[k.id].default])) as SettingsPatch);

  const workers = (id: IntensityId) => settings?.tiers.find((t) => t.id === id)?.workers;
</script>

<main>
  <div class="top">
    <button class="secondary" onclick={onBack}>← Back</button>
    <h1>Options</h1>
  </div>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#if !settings}
    {#if !error}<p class="hint">Loading…</p>{/if}
  {:else}
    <section>
      <h2>Scan speed</h2>
      <p class="hint">
        How hard a scan may push this computer. It applies to every scan from now on, and is remembered with your data. This machine has
        {settings.logical_cores} cores.
      </p>
      <div class="tiers" role="radiogroup" aria-label="Scan speed">
        {#each tiers as t (t.id)}
          <label class="tier" class:on={settings.scan_intensity === t.id}>
            <input type="radio" name="intensity" value={t.id} checked={settings.scan_intensity === t.id} onchange={() => choose(t.id)} />
            <span class="body">
              <strong>{t.title}{t.id === 'default' ? ' (recommended)' : ''}</strong>
              <span class="hint">{workers(t.id)} worker{workers(t.id) === 1 ? '' : 's'}</span>
              <span>{t.blurb}</span>
            </span>
          </label>
        {/each}
      </div>
      <p class="status" aria-live="polite">{saved ? 'Saved.' : ' '}</p>
    </section>

    <details class="advanced">
      <summary><h2>Advanced matching</h2></summary>
      <p class="hint">
        How strictly the scan groups photos. The defaults were tuned on real libraries and suit most people. Changes apply to the <strong>next
        scan</strong>; scan the folder again to see them.
      </p>
      {#each knobs as k (k.id)}
        {@const kn = settings.advanced[k.id]}
        <div class="knob">
          <div class="knob-head">
            <label for={k.id}><strong>{k.title}</strong></label>
            <span class="value">{shown(k.id)}{k.unit}</span>
            {#if kn.value !== kn.default}
              <button class="link" onclick={() => save({ [k.id]: kn.default })}>Reset to {kn.default}{k.unit}</button>
            {/if}
          </div>
          <p class="hint">{k.blurb}</p>
          <div class="slider">
            <span class="hint">{k.low}</span>
            <input
              id={k.id}
              type="range"
              min={kn.min}
              max={kn.max}
              step={k.step}
              value={kn.value}
              oninput={(e) => (drag[k.id] = Number(e.currentTarget.value))}
              onchange={(e) => save({ [k.id]: Number(e.currentTarget.value) })}
            />
            <span class="hint">{k.high}</span>
          </div>
          {#if k.caution}<p class="caution">{k.caution}</p>{/if}
        </div>
      {/each}
      <button class="secondary" disabled={allDefault} onclick={resetAll}>Reset all to defaults</button>
    </details>

    <p class="hint">
      Choices that belong to one scan or one screen live there instead: "Include videos" is on the start screen, and the sort order is on the
      review screen.
    </p>
  {/if}
</main>

<style>
  .top {
    display: flex;
    align-items: center;
    gap: 1rem;
    margin-bottom: 1.5rem;
  }
  .top h1 {
    margin: 0;
  }
  h2 {
    font-size: 1.05rem;
    margin: 0 0 0.25rem;
  }
  .tiers {
    display: grid;
    gap: 0.6rem;
    margin: 0.75rem 0 0.25rem;
  }
  .tier {
    display: flex;
    gap: 0.75rem;
    align-items: flex-start;
    padding: 0.75rem 0.9rem;
    border: 1px solid var(--line);
    border-radius: 10px;
    cursor: pointer;
  }
  .tier.on {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }
  .tier input {
    margin-top: 0.35rem;
  }
  .body {
    display: grid;
    gap: 0.2rem;
  }
  .advanced {
    border: 1px solid var(--line);
    border-radius: 10px;
    padding: 0.75rem 1rem;
    margin: 0 0 1.25rem;
  }
  .advanced summary {
    cursor: pointer;
  }
  .advanced summary h2 {
    display: inline;
  }
  .knob {
    padding: 0.75rem 0;
    border-top: 1px solid var(--line);
  }
  .knob-head {
    display: flex;
    align-items: baseline;
    gap: 0.75rem;
  }
  .knob-head .value {
    font-variant-numeric: tabular-nums;
    color: var(--accent);
    font-weight: 600;
  }
  .knob p {
    margin: 0.25rem 0;
  }
  .slider {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 0.75rem;
  }
  .slider input {
    width: 100%;
    accent-color: var(--accent);
  }
  .caution {
    font-size: 0.85em;
    color: var(--danger);
  }
  .status {
    color: var(--ok);
    min-height: 1.5em;
    margin: 0.25rem 0 1rem;
  }
</style>
