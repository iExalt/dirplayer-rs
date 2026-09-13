import assert from "node:assert/strict";

// test: Run against a loaded Director fixture with two distinct, short sound members.
// The caller owns Chromium and resource serving; no copyrighted fixture is bundled.
export async function testSoundPlayback(page, member, otherMember) {
  await page.evaluate(() => {
    vm.stop();
    window.soundTestStarts = [];
    const start = AudioBufferSourceNode.prototype.start;
    AudioBufferSourceNode.prototype.start = function (...args) {
      const result = start.apply(this, args);
      window.soundTestStarts.push({ rate: this.playbackRate.value, duration: this.buffer?.duration });
      return result;
    };
  });
  const evaluate = async code => {
    const result = await page.evaluate(async code => JSON.parse(await vm.mcp_eval_lingo(code)), code);
    assert.equal(result.success, true, `${code}: ${JSON.stringify(result)}`);
    return result;
  };
  for (let channel = 1; channel <= 8; channel++) await evaluate(`sound(${channel}).stop()`);
  const reset = async () => {
    await evaluate("sound(1).stop()");
    await page.evaluate(() => { soundTestStarts.length = 0; });
  };
  const started = async count => {
    await page.waitForFunction(count => soundTestStarts.length >= count, count, {timeout: 15000});
    return page.evaluate(() => soundTestStarts);
  };
  const queue = (name, shift) => evaluate(`sound(1).queue([#member: member(${JSON.stringify(name)}), #rateShift: ${shift}])`);
  const outcomes = [];
  for (const shift of [0, -2, 12]) {
    await reset();
    await queue(member, shift);
    await evaluate("sound(1).play()");
    const [source] = await started(1);
    assert.ok(Math.abs(source.rate - 2 ** (shift / 12)) < 1e-6);
    outcomes.push({test: "pitch", shift, ...source});
  }
  await reset();
  await queue(member, -12);
  await evaluate("sound(1).play()");
  await started(1);
  await queue(otherMember, 12);
  const entries = await started(2);
  assert.equal(entries[0].rate, 0.5);
  assert.equal(entries[1].rate, 2);
  assert.notEqual(entries[0].duration, entries[1].duration, "new entry must not reuse old buffer");
  outcomes.push({test: "queue-transition", entries});

  await reset();
  await evaluate(`sound(1).setPlayList([[#member: member(${JSON.stringify(member)}), #rateShift: -12], [#member: member(${JSON.stringify(otherMember)}), #rateShift: 12]])`);
  await evaluate("sound(1).play()");
  const playlist = await started(2);
  assert.deepEqual(playlist.map(s => s.rate), [0.5, 2]);
  outcomes.push({test: "playlist-transition", entries: playlist});

  await reset();
  await evaluate(`sound(1).queue([#member: member(${JSON.stringify(member)}), #rateShift: -2, #loopCount: 2])`);
  await evaluate("sound(1).play()");
  const loops = await started(2);
  assert.ok(loops.every(s => Math.abs(s.rate - 2 ** (-2 / 12)) < 1e-6));
  outcomes.push({test: "repeat-retains-pitch", entries: loops});

  await reset();
  await evaluate(`sound(1).play(member(${JSON.stringify(member)}))`);
  assert.equal((await started(1))[0].rate, 1, "bare member must reset pitch");
  outcomes.push({test: "bare-member-resets-pitch"});
  await reset();
  const invalid = await page.evaluate(async name => JSON.parse(await vm.mcp_eval_lingo(`sound(1).queue([#member: member(${JSON.stringify(name)}), #rateShift: "invalid"])`)), member);
  assert.equal(invalid.success, false, "invalid pitch must reject the entry");
  await queue("__missing_sound_regression__", 0);
  await evaluate("sound(1).play()");
  await page.waitForTimeout(100);
  assert.equal((await page.evaluate(() => soundTestStarts.length)), 0);
  await reset();
  await queue(member, 0);
  await evaluate("sound(1).play()");
  assert.equal((await started(1))[0].rate, 1);
  outcomes.push({test: "invalid-entry-and-missing-member-recovery"});
  await reset();
  return outcomes;
}
