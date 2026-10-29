<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError, type Counts, type IntensityId, type KnobId, type Settings, type SettingsPatch } from './lib/api';
  import { describeChange } from './lib/knobs';
  import Icon from './Icon.svelte';

  // `scanId` is the scan in the database, if there is one: sliders preview on it and apply to it.
  let { onBack, scanId = null }: { onBack: () => void; scanId?: string | null } = $props();

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

  // What the current results would look like with the value under the
  // finger, worked out on the server from the saved analysis. `seq` drops
  // answers that arrive after a newer question.
  let preview = $state<{ knob: KnobId; lines: string[] } | { knob: KnobId; message: string } | null>(null);
  let seq = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  // True once the server says this scan predates trying settings on it.
  let cannotRegroup = $state(false);

  function previewDrag(knob: KnobId, value: number) {
    drag[knob] = value;
    if (!scanId || !settings || cannotRegroup) return;
    clearTimeout(timer);
    const values = Object.fromEntries(knobs.map((k) => [k.id, k.id === knob ? value : (drag[k.id] ?? settings!.advanced[k.id].value)])) as Record<KnobId, number>;
    const mine = ++seq;
    timer = setTimeout(async () => {
      try {
        const p = await api.previewRegroup(scanId, values);
        if (mine !== seq) return;
        const lines = describeChange(knob, p.current, p.proposed);
        preview = lines.length ? { knob, lines } : { knob, message: 'No change to your current results.' };
      } catch (e) {
        if (mine !== seq) return;
        if (e instanceof ApiError && e.status === 409) cannotRegroup = true;
        preview = null;
      }
    }, 120);
  }

  /** Saves a matching value, then regroups the current review with it. */
  async function saveKnob(knob: KnobId, value: number) {
    clearTimeout(timer);
    seq++;
    await save({ [knob]: value });
    if (error || !scanId || cannotRegroup) return;
    try {
      const c: Counts = await api.regroup(scanId);
      appliedKnob = knob;
      applied = `Your review is updated: ${c.sequences} quick sequence${c.sequences === 1 ? '' : 's'}, ${c.duplicates + c.exact} duplicate groups, ${c.look_alikes} look-alike pairs.`;
      preview = null;
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) cannotRegroup = true;
      else error = say(e);
    }
  }
  let applied = $state('');
  // Which slider the confirmation belongs to (null: it was a reset of all of them).
  let appliedKnob = $state<KnobId | null>(null);

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
    applied = '';
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
  const resetAll = async () => {
    if (!settings) return;
    await save(Object.fromEntries(knobs.map((k) => [k.id, settings!.advanced[k.id].default])) as SettingsPatch);
    if (!error && scanId && !cannotRegroup) await regroupNow();
  };
  async function regroupNow() {
    try {
      await api.regroup(scanId!);
      appliedKnob = null;
      applied = 'Your review is updated.';
      preview = null;
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) cannotRegroup = true;
      else error = say(e);
    }
  }

  const workers = (id: IntensityId) => settings?.tiers.find((t) => t.id === id)?.workers;

  import { applyTheme, storedTheme, type Theme } from './lib/theme';
  let theme = $state<Theme>(storedTheme());
  const themes: [Theme, string][] = [['system', 'System'], ['light', 'Light'], ['dark', 'Dark']];
  const pickTheme = (t: Theme) => {
    theme = t;
    applyTheme(t);
  };
</script>

<main>
  <header class="top">
    <button class="btn" onclick={onBack}><Icon name="chevron-left" /> Back</button>
    <h1 class="title">Settings</h1>
    <span class="status" aria-live="polite">{#if saved}<Icon name="check" size={14} /> Saved{/if}</span>
  </header>

  <div class="column">
    {#if error}<div class="notice danger" role="alert"><Icon name="info" size={18} /><p>{error}</p></div>{/if}

    {#if !settings}
      {#if !error}
        <div class="skeleton" style="height:220px"></div>
      {/if}
    {:else}
      <section>
        <h2 class="lead">Appearance</h2>
        <p class="hint intro">System follows your computer's light or dark setting.</p>
        <div class="seg" role="group" aria-label="Appearance">
          {#each themes as [key, label] (key)}
            <button aria-pressed={theme === key} onclick={() => pickTheme(key)}>{label}</button>
          {/each}
        </div>
      </section>

      <section>
        <h2 class="lead">Scan speed</h2>
        <p class="hint intro">
          How hard a scan may push this computer. It applies to every scan from now on and is remembered with your data. This machine has
          {settings.logical_cores} cores.
        </p>
        <div class="tiers" role="radiogroup" aria-label="Scan speed">
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
        </div>
      </section>

      <details class="advanced">
        <summary><span class="lead">Advanced matching</span><Icon name="chevron-right" size={16} /></summary>
        <p class="hint intro">
          How strictly photos are grouped. The defaults were tuned on real libraries and suit most people.
          {#if scanId && !cannotRegroup}
            Drag a slider to see what it would change in your current review; letting go applies it. Your keep and quarantine choices are kept.
          {:else}
            Changes apply to the <strong>next scan</strong>; scan the folder again to see them.
          {/if}
        </p>
        {#each knobs as k (k.id)}
          {@const kn = settings.advanced[k.id]}
          <div class="knob">
            <div class="knob-head">
              <label for={k.id}><strong>{k.title}</strong></label>
              <span class="value">{shown(k.id)}{k.unit}</span>
              {#if kn.value !== kn.default}
                <button class="link" onclick={() => saveKnob(k.id, kn.default)}>Reset to {kn.default}{k.unit}</button>
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
                style="--p: {((shown(k.id) - kn.min) / (kn.max - kn.min)) * 100}%"
                oninput={(e) => previewDrag(k.id, Number(e.currentTarget.value))}
                onchange={(e) => saveKnob(k.id, Number(e.currentTarget.value))}
              />
              <span class="hint">{k.high}</span>
            </div>
            <div class="effect" aria-live="polite">
              {#if preview && preview.knob === k.id}
                {#if 'lines' in preview}
                  <span class="hint">With this value:</span>
                  <ul>{#each preview.lines as l (l)}<li>{l}</li>{/each}</ul>
                {:else}
                  <span class="hint">{preview.message}</span>
                {/if}
              {:else if applied && appliedKnob === k.id}
                <p class="applied"><Icon name="check" size={15} /><span>{applied}</span></p>
              {/if}
            </div>
            {#if k.caution}<p class="notice warning small"><Icon name="info" size={15} /><span>{k.caution}</span></p>{/if}
          </div>
        {/each}
        {#if applied && appliedKnob === null}<p class="notice success small" role="status"><Icon name="check" size={15} /><span>{applied}</span></p>{/if}
        <button class="btn" disabled={allDefault} onclick={resetAll}>Reset all to defaults</button>
      </details>

      <p class="hint">
        Choices that belong to one scan or one screen live there instead: “Include videos” is on the start screen, and the sort order is on the
        review screen.
      </p>
    {/if}
  </div>
</main>

<style>
  .top {
    display: flex;
    align-items: center;
    gap: var(--s4);
    padding: var(--s3) var(--s4);
    min-height: 54px;
  }
  .status {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: var(--s1);
    color: var(--success);
    font-weight: 600;
    min-height: 1.5em;
  }
  .column {
    width: min(680px, 100%);
    margin: 0 auto;
    padding: var(--s2) var(--s4) var(--s7);
    display: grid;
    gap: var(--s5);
  }
  .intro {
    margin: var(--s1) 0 var(--s3);
    max-width: 70ch;
  }
  .tiers {
    display: grid;
    gap: var(--s2);
  }
  .tier {
    display: flex;
    gap: var(--s3);
    align-items: flex-start;
    padding: var(--s3) var(--s4);
    border-radius: var(--radius);
    background: var(--surface);
    box-shadow: var(--shadow-sm);
    cursor: pointer;
    transition: box-shadow var(--t-fast) var(--ease);
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
    cursor: pointer;
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
    gap: var(--s2);
  }
  .advanced {
    border-radius: var(--radius);
    background: var(--surface);
    box-shadow: var(--shadow-sm);
    padding: var(--s3) var(--s4);
  }
  .advanced summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    cursor: pointer;
    list-style: none;
  }
  .advanced summary::-webkit-details-marker {
    display: none;
  }
  .advanced summary :global(svg) {
    transition: transform var(--t) var(--ease);
    color: var(--label-3);
  }
  .advanced[open] summary :global(svg) {
    transform: rotate(90deg);
  }
  .advanced[open] {
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
    gap: var(--s3);
  }
  .effect {
    min-height: 1.5em;
    padding-bottom: var(--s2);
  }
  .applied {
    display: flex;
    align-items: center;
    gap: var(--s2);
    margin: 0;
    color: var(--success);
    font-size: var(--fs-small);
  }
  .effect ul {
    margin: var(--s1) 0 0;
    padding-left: var(--s4);
    font-variant-numeric: tabular-nums;
  }
  .small {
    font-size: var(--fs-small);
    padding: var(--s2) var(--s3);
  }
</style>
