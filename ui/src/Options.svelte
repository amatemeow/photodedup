<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError, type IntensityId, type Settings } from './lib/api';

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

  const say = (e: unknown) => (e instanceof ApiError ? e.message : e instanceof Error ? e.message : String(e));

  onMount(async () => {
    try {
      settings = await api.settings();
    } catch (e) {
      error = say(e);
    }
  });

  async function choose(id: IntensityId) {
    if (!settings || settings.scan_intensity === id) return;
    error = '';
    saved = false;
    try {
      settings = await api.saveSettings(id);
      saved = true;
    } catch (e) {
      error = say(e);
    }
  }

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
  .status {
    color: var(--ok);
    min-height: 1.5em;
    margin: 0.25rem 0 1rem;
  }
</style>
