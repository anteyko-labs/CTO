import { test } from 'node:test'
import assert from 'node:assert/strict'
import { isPersonalChat, isService, jidOf, makeDebouncer, phoneOf, textOf, typingMs } from '../lib.mjs'

test('номер и jid', () => {
  assert.equal(jidOf('+996 555 12-34-56'), '996555123456@s.whatsapp.net')
  assert.equal(jidOf('12'), null)
  assert.equal(phoneOf('996555123456@s.whatsapp.net'), '+996555123456')
  assert.equal(phoneOf('996555123456:12@s.whatsapp.net'), '+996555123456')
})

test('только личные чаты', () => {
  assert.ok(isPersonalChat('996555123456@s.whatsapp.net'))
  assert.ok(!isPersonalChat('1203630@g.us'))
  assert.ok(!isPersonalChat('status@broadcast'))
  assert.ok(!isPersonalChat('1203@newsletter'))
})

test('текст, медиа и служебные', () => {
  assert.equal(textOf({ conversation: '1' }), '1')
  assert.equal(textOf({ extendedTextMessage: { text: 'привет' } }), 'привет')
  assert.equal(textOf({ ephemeralMessage: { message: { conversation: '2' } } }), '2')
  assert.equal(textOf({ imageMessage: {} }), null)
  assert.ok(isService({ reactionMessage: {} }))
  assert.ok(isService({ protocolMessage: {} }))
  assert.ok(!isService({ conversation: 'x' }))
})

test('пауза «печатает…»', () => {
  assert.equal(typingMs('1'), 1000)
  assert.equal(typingMs('x'.repeat(500)), 4000)
})

test('склейка сообщений подряд', async () => {
  const got = []
  const d = makeDebouncer(30, (from, text, media) => got.push([from, text, media]))
  d.push('+1', 'добрый день', false)
  d.push('+1', 'сколько долг?', false)
  d.push('+2', null, true)
  await new Promise((r) => setTimeout(r, 60))
  assert.deepEqual(got.sort(), [['+1', 'добрый день\nсколько долг?', false], ['+2', '', true]].sort())
  assert.equal(d.size(), 0)
})
