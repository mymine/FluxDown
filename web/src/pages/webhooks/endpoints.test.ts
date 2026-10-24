import { describe, expect, test } from 'bun:test'
import { CLOUD_EVENTS, WEBHOOK_EVENTS } from './endpoints'

describe('webhook event catalog', () => {
  test('local webhooks can subscribe queue.started', () => {
    expect(WEBHOOK_EVENTS.some((event) => event.wire === 'queue.started')).toBe(true)
  })

  test('cloud channels drop only the local-only queue.started and keep order', () => {
    expect(CLOUD_EVENTS.some((event) => event.wire === 'queue.started')).toBe(false)
    expect(CLOUD_EVENTS.map((event) => event.wire)).toEqual(
      WEBHOOK_EVENTS.map((event) => event.wire).filter((wire) => wire !== 'queue.started'),
    )
  })
})
