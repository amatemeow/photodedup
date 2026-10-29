<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError, type IntensityId, type Settings, type SettingsPatch } from './lib/api';
  import Icon from './Icon.svelte';

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

      <p class="hint">
        The matching sliders (how alike photos must look, and how close in time) are in the review itself, under <strong>Fine-tune</strong>, so you
        can watch the results change as you move them.
      </p>

      <p class="hint">
        “Include videos” is on the start screen, and the sort order is on the review screen.
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
</style>
