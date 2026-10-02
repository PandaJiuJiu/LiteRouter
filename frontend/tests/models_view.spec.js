import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ElementPlus from 'element-plus'
import { createRouter, createMemoryHistory } from 'vue-router'
import i18n, { setLocale } from '../src/i18n'
import zhCN from '../src/i18n/locales/zh-CN'

const listChannels = vi.fn()
const testModel = vi.fn()
const updateChannelModels = vi.fn()
const fetchModels = vi.fn()
vi.mock('../src/api', () => ({ listChannels, testModel, updateChannelModels, fetchModels }))

const { default: Models } = await import('../src/views/Models.vue')

/** One channel serving a single model over the OpenAI-compatible URL. */
const CHANNEL = {
  id: 1,
  name: 'ark',
  base_url: 'https://ark.example',
  base_url_anthropic: '',
  models: 'gpt-4o',
  disabled_models: '',
  enabled: 1,
}

async function mountModels() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }],
  })
  const w = mount(Models, { global: { plugins: [ElementPlus, i18n, router] } })
  await flushPromises()
  return w
}

/** The per-protocol status lines on the card for `model`, as `{ ok, text }`. */
const statusLines = (w, model = 'gpt-4o') =>
  w.findAll('.model-card').find((c) => c.find('.model-name').text() === model)
    .findAll('.status-line')
    .map((l) => ({ ok: l.classes().includes('ok'), text: l.text() }))

beforeEach(() => {
  listChannels.mockReset()
  testModel.mockReset()
  updateChannelModels.mockReset()
  fetchModels.mockReset()
  setLocale('zh-CN')
  listChannels.mockResolvedValue([CHANNEL])
})

describe('model test result', () => {
  it('shows the round-trip latency instead of the word "Available"', async () => {
    // The word was redundant with the check icon; the latency is the part the
    // admin actually wanted — which of the two protocols is the slow one.
    testModel.mockResolvedValue({ ok: true, protocols: { openai: { ok: true, ms: 842 } } })
    const w = await mountModels()
    await w.find('.model-card .action-btn').trigger('click')
    await flushPromises()

    expect(statusLines(w)).toEqual([{ ok: true, text: 'openai842 ms' }])
  })

  it('keeps availability in the icon, not the text, when the probe fails', async () => {
    testModel.mockResolvedValue({
      ok: false,
      protocols: { openai: { ok: false, ms: 120, error: 'HTTP 401: invalid api key' } },
    })
    const w = await mountModels()
    await w.find('.model-card .action-btn').trigger('click')
    await flushPromises()

    const [line] = statusLines(w)
    expect(line.ok).toBe(false)
    expect(line.text).toContain('120 ms')
    expect(line.text).not.toContain(zhCN.models.available)
  })

  it('switches to seconds above a second so a timeout is readable at a glance', async () => {
    testModel.mockResolvedValue({ ok: true, protocols: { openai: { ok: true, ms: 10_040 } } })
    const w = await mountModels()
    await w.find('.model-card .action-btn').trigger('click')
    await flushPromises()

    expect(statusLines(w)[0].text).toContain('10.04 s')
  })

  it('falls back to a dash when the backend sends no latency', async () => {
    // An older backend, or a transport error caught client-side, leaves `ms`
    // undefined — the card must still render rather than show "undefined ms".
    testModel.mockResolvedValue({ ok: true, protocols: { openai: { ok: true } } })
    const w = await mountModels()
    await w.find('.model-card .action-btn').trigger('click')
    await flushPromises()

    expect(statusLines(w)[0].text).toContain('—')
  })

  it('reports both protocols side by side when both are tested', async () => {
    testModel.mockResolvedValue({
      ok: true,
      protocols: {
        openai: { ok: true, ms: 300 },
        anthropic: { ok: true, ms: 2_500 },
      },
    })
    const w = await mountModels()
    await w.find('.model-card .action-btn').trigger('click')
    await flushPromises()

    expect(statusLines(w).map((l) => l.text)).toEqual([
      'openai300 ms',
      'anthropic2.50 s',
    ])
  })
})

describe('test all', () => {
  // A model is in exactly one of the two lists — toggleModel deletes it from
  // the other — so the fixture must keep them disjoint or isSelected() reads
  // the enabled list and the test proves nothing.
  const channelWith = (models, disabled_models) => ({ ...CHANNEL, models, disabled_models })

  /** The model names each `testModel` call asked for, in call order. */
  const tested = () => testModel.mock.calls.map((c) => c[0].model)

  /** The channel-level "test all" button — not the toolbar's refresh. */
  const testAllBtn = (w) =>
    w.findAll('.el-button').find((b) => b.text().includes(zhCN.models.testAll))

  it('skips disabled models', async () => {
    // A disabled model is not routed to, so probing it tells the admin nothing
    // — and on a channel with many disabled models it stretches "test all" to
    // minutes of waiting for cards that are greyed out anyway.
    listChannels.mockResolvedValue([channelWith('gpt-4o', 'gpt-4o-mini')])
    testModel.mockResolvedValue({ ok: true, protocols: { openai: { ok: true, ms: 100 } } })
    const w = await mountModels()

    await testAllBtn(w).trigger('click')
    await flushPromises()

    expect(tested()).toEqual(['gpt-4o'])
  })

  it('still leaves disabled cards untested rather than marked failed', async () => {
    // The skipped card must keep saying "Not tested": a red ✗ on a model the
    // admin deliberately switched off would read as a real fault.
    listChannels.mockResolvedValue([channelWith('gpt-4o', 'gpt-4o-mini')])
    testModel.mockResolvedValue({ ok: true, protocols: { openai: { ok: true, ms: 100 } } })
    const w = await mountModels()

    await testAllBtn(w).trigger('click')
    await flushPromises()

    const skipped = w.findAll('.model-card')
      .find((c) => c.find('.model-name').text() === 'gpt-4o-mini')
    expect(skipped.find('.status-untested').text()).toBe(zhCN.models.untested)
  })

  it('tests every model when they are all enabled', async () => {
    listChannels.mockResolvedValue([channelWith('gpt-4o,gpt-4o-mini,gpt-5', '')])
    testModel.mockResolvedValue({ ok: true, protocols: { openai: { ok: true, ms: 100 } } })
    const w = await mountModels()

    await testAllBtn(w).trigger('click')
    await flushPromises()

    expect(tested()).toEqual(['gpt-4o', 'gpt-4o-mini', 'gpt-5'])
  })

  it('disables the button when every model is switched off', async () => {
    // Otherwise the button is clickable but does nothing, which reads as a bug.
    listChannels.mockResolvedValue([channelWith('', 'gpt-4o')])
    const w = await mountModels()

    expect(testAllBtn(w).classes()).toContain('is-disabled')
    await testAllBtn(w).trigger('click')
    await flushPromises()
    expect(testModel).not.toHaveBeenCalled()
  })
})

describe('toolbar filters', () => {
  // Three channels with disjoint model names: a search for "gpt" must surface
  // ark + azure but not mistral; "enabled only" must keep the two channels
  // that still route traffic (ark, azure) and hide the disabled-only channel
  // (gpt-channels-azure is half disabled so it stays; mistral-disabled-gram
  // has no enabled model so it gets hidden).
  // Wait, for `enabledOnly`, the rule is: the channel must have at least one
  // enabled model. ark=1 enabled, azure=1 enabled, mistral=1 disabled → only
  // ark and azure pass.
  const override = (overrides) => ({ ...CHANNEL, ...overrides })
  const CHANNELS = () => [
    override({ id: 1, name: 'ark', models: 'gpt-4o', disabled_models: '' }),
    override({ id: 2, name: 'azure', models: 'gpt-4o-mini', disabled_models: 'gpt-4' }),
    override({ id: 3, name: 'mistral', models: '', disabled_models: 'mistral-large' }),
  ]

  const visibleCards = (w) =>
    w.findAll('.channel-card').map((c) => c.find('.channel-name').text())

  beforeEach(() => {
    listChannels.mockResolvedValue(CHANNELS())
  })

  it('summary counts models across every channel regardless of filter', async () => {
    // 1 + 2 + 1 = 4 models (enabled + disabled, deduped), 3 channels,
    // 2 disabled — the chips reflect the *whole* DB, not the filtered view,
    // so they don't flicker while the admin types in the search box.
    const w = await mountModels()
    await flushPromises()

    const text = w.find('.summary').text()
    expect(text).toContain(zhCN.models.summaryModels.replace('{count}', '4'))
    expect(text).toContain(zhCN.models.summaryChannels.replace('{count}', '3'))
    expect(text).toContain(zhCN.models.summaryDisabled.replace('{count}', '2'))
  })

  it('search filters channels whose models match (case-insensitive)', async () => {
    // A "GPT" search must surface the two gpt-* channels but not mistral.
    // "gpt-4" lives in azure's disabled list, so that channel shows up too —
    // the filter is across both lists, not just the enabled set.
    const w = await mountModels()
    await flushPromises()

    await w.find('.filter-search input').setValue('GPT')
    await flushPromises()

    const visible = visibleCards(w)
    expect(visible).toContain('ark')
    expect(visible).toContain('azure')
    expect(visible).not.toContain('mistral')
  })

  it('enabledOnly hides channels that have no enabled model', async () => {
    // mistral's only model is disabled — the channel routes nothing, so
    // "enabled only" hides it. ark and azure both have an enabled entry,
    // so they stay. This is the inverse of the previous "disabled only"
    // toggle: pick the one that matches what you actually want to see.
    const w = await mountModels()
    await flushPromises()

    await w.find('.filter-toggle .el-switch').trigger('click')
    await flushPromises()

    const visible = visibleCards(w)
    expect(visible).toContain('ark')
    expect(visible).toContain('azure')
    expect(visible).not.toContain('mistral')
  })
})