#!/usr/bin/env python3
"""Live audio integration check using only disposable virtual audio devices.

Build with cargo build, then run this inside a PipeWire/PulseAudio session.
Requires pactl, gst-launch-1.0, ffmpeg, and ffprobe. No physical mic is captured.
"""

import array
import json
import math
import os
from pathlib import Path
import subprocess
import tempfile
import time

binary = str(Path(__file__).resolve().parent.parent / 'target/debug/mentarch-recorder')
prefix = f'mentarch_test_{os.getpid()}'
modules = []
players = []
process = None

def pactl(*args):
    return subprocess.check_output(['pactl', *args], text=True).strip()

def magnitude(samples, frequency):
    samples = samples[48000:96000]
    step = 2 * math.pi * frequency / 48000
    a = sum(value * math.cos(step * i) for i, value in enumerate(samples))
    b = sum(value * math.sin(step * i) for i, value in enumerate(samples))
    return 2 * math.hypot(a, b) / len(samples)

try:
    for kind in ('mic', 'desktop'):
        name = f'{prefix}_{kind}'
        modules.append(pactl('load-module', 'module-null-sink', f'sink_name={name}', 'rate=48000', 'channels=2', f'sink_properties=device.description=RecorderTest_{kind} priority.session=1'))
    mic = f'{prefix}_input'
    modules.append(pactl('load-module', 'module-remap-source', f'master={prefix}_mic.monitor', f'source_name={mic}', 'source_properties=device.description=RecorderTest_Microphone priority.session=1'))
    pactl('set-source-volume', mic, '100%')
    for kind in ('mic', 'desktop'):
        pactl('set-sink-volume', f'{prefix}_{kind}', '100%')
    for kind, frequency in [('mic', 440), ('desktop', 880)]:
        players.append(subprocess.Popen(['gst-launch-1.0', '-q', 'audiotestsrc', 'is-live=true', f'freq={frequency}', 'volume=0.4', '!', 'audio/x-raw,rate=48000,channels=2', '!', 'pulsesink', f'device={prefix}_{kind}', 'volume=1.0'], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE))
    time.sleep(.4)
    with tempfile.TemporaryDirectory(prefix='mentarch-audio-config-') as config:
        env = dict(os.environ, XDG_CONFIG_HOME=config)
        path = Path(config)/'mentarch-recorder/audio.json'
        path.parent.mkdir()
        output = Path(config) / 'recordings'
        output.mkdir()
        for microphone, desktop in [(True, False), (False, True), (True, True)]:
            path.write_text(json.dumps({'microphone':microphone,'desktop':desktop,'microphone_device':mic,'desktop_device':f'{prefix}_desktop'}))
            result = subprocess.run([binary, '--audio-device-test', str(output)],env=env,text=True,capture_output=True,timeout=20)
            assert result.returncode == 0, result.stdout + result.stderr
            video = result.stdout.split('Saved audio device test: ')[1].strip()
            info = json.loads(subprocess.check_output(['ffprobe','-v','error','-show_streams','-show_format','-of','json',video]))
            assert [s['codec_type'] for s in info['streams']] == ['video','audio'], info
            packets = json.loads(subprocess.check_output(['ffprobe','-v','error','-show_packets','-show_entries','packet=stream_index,pts_time,duration_time','-of','json',video]))['packets']
            timelines = [[p for p in packets if p['stream_index'] == i] for i in (0,1)]
            starts = [float(p[0]['pts_time']) for p in timelines]
            ends = [float(p[-1]['pts_time']) + float(p[-1].get('duration_time',0)) for p in timelines]
            assert abs(starts[0] - starts[1]) < .25, starts
            assert abs(ends[0] - ends[1]) < .3, ends
            raw = subprocess.check_output(['ffmpeg','-v','error','-i',video,'-map','0:a:0','-ac','1','-ar','48000','-f','f32le','pipe:1'])
            samples = array.array('f'); samples.frombytes(raw)
            amplitudes = [magnitude(samples, f) for f in (440,880)]
            assert (amplitudes[0] > .005) if microphone else (amplitudes[0] < .001), amplitudes
            assert (amplitudes[1] > .005) if desktop else (amplitudes[1] < .001), amplitudes
            assert max(map(abs,samples)) < .95, 'Clipped mixture'
            subprocess.run(['ffmpeg','-v','error','-i',video,'-f','null','-'],check=True)
            print(json.dumps({'microphone':microphone,'desktop':desktop,'tone_amplitudes':amplitudes,'duration':info['format']['duration'],'file':video}), flush=True)
        # Simulate the server rerouting an active input to a different device.
        path.write_text(json.dumps({'microphone':True,'microphone_device':mic}))
        process = subprocess.Popen([binary,'--audio-device-test',str(output)],env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        time.sleep(1.0)
        outputs=json.loads(pactl('--format=json','list','source-outputs'))
        owned=[s for s in outputs if s.get('properties',{}).get('application.process.id')==str(process.pid)]
        assert len(owned)==1, owned
        move=subprocess.run(['pactl','move-source-output',str(owned[0]['index']),f'{prefix}_desktop.monitor'],capture_output=True,text=True)
        stdout,stderr = process.communicate(timeout=15)
        if move.returncode != 0:
            assert process.returncode == 0, stderr
            print('Pinned capture: server rejected source substitution.', flush=True)
        else:
            assert process.returncode != 0, 'Server routing silently substituted another microphone'
            assert 'disconnected or changed' in stderr, stderr
            print('Unexpected route rejected: ' + stderr.strip(), flush=True)
finally:
    if process is not None and process.poll() is None:
        process.terminate()
        process.communicate(timeout=5)
    for process in players:
        process.terminate()
        try: process.communicate(timeout=3)
        except subprocess.TimeoutExpired: process.kill(); process.communicate()
    for module in reversed(modules):
        subprocess.run(['pactl','unload-module',module],capture_output=True)
