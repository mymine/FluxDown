import { expect, test } from 'bun:test'
import { POST_ONLY_ENDPOINT_MESSAGE, POST_ONLY_RESUME_MESSAGE, localizeTaskError, taskErrorKey } from './taskError'

test('引擎稳定错误消息映射到 i18n key，其余原样', () => {
  expect(taskErrorKey(POST_ONLY_ENDPOINT_MESSAGE)).toBe('taskErrorPostOnlyEndpoint')
  expect(taskErrorKey(`  ${POST_ONLY_RESUME_MESSAGE}\n`)).toBe('taskErrorPostOnlyResume')
  expect(taskErrorKey('connection reset')).toBeNull()
  expect(taskErrorKey(`${POST_ONLY_ENDPOINT_MESSAGE} extra`)).toBeNull()
  expect(localizeTaskError((key) => `L:${key}`, POST_ONLY_ENDPOINT_MESSAGE)).toBe('L:taskErrorPostOnlyEndpoint')
  expect(localizeTaskError((key) => `L:${key}`, 'connection reset')).toBe('connection reset')
})
