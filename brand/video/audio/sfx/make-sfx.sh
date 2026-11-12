#!/usr/bin/env bash
# Regenerates every bleh.xyz sound effect from scratch with ffmpeg. No samples, no downloads.
#
#   brand/video/audio/sfx/make-sfx.sh            # all effects
#   brand/video/audio/sfx/make-sfx.sh tick blip  # only some
#   CHECK=1 brand/video/audio/sfx/make-sfx.sh    # also write waveform + spectrogram PNGs to sfx/check/
#
# Output: 48 kHz, 16-bit, mono WAV next to this script. Mono on purpose: the mixer pans each cue.
# Deterministic: every sound is a closed-form expression of time (aevalsrc). Noise comes from
# aevalsrc's random(0), which is a fixed-seed LCG, and files are written with -bitexact, so a
# re-run with the same ffmpeg build gives byte-identical files.
#
# Each effect is peak-normalised (second argument of `gen`, dBFS) so cue gains in the cue sheets
# mean the same thing for every file.
set -euo pipefail
cd "$(dirname "$0")"
SR=48000
T="2*PI"
N="(random(0)*2-1)"          # white noise, fixed seed
want=("$@")

# gen <name> <peak dBFS> <seconds> <filtergraph ending in [o]>
gen() {
  local name=$1 peak=$2 dur=$3 graph=$4
  if ((${#want[@]})) && [[ ! " ${want[*]} " == *" $name "* ]]; then return; fi
  local tmp; tmp=$(mktemp -t blehsfx).wav
  ffmpeg -y -v error -filter_complex "$graph" -map '[o]' -t "$dur" -ar $SR -ac 1 -c:a pcm_f32le "$tmp"
  local max; max=$(ffmpeg -v info -i "$tmp" -af volumedetect -f null - 2>&1 | sed -n 's/.*max_volume: \(.*\) dB/\1/p')
  local g; g=$(awk -v m="$max" -v p="$peak" 'BEGIN{printf "%.2f", p-m}')
  local fo; fo=$(awk -v d="$dur" 'BEGIN{printf "%.3f", d-0.012}')
  ffmpeg -y -v error -i "$tmp" -af "volume=${g}dB,afade=t=in:d=0.001,afade=t=out:st=${fo}:d=0.012" \
    -ar $SR -ac 1 -c:a pcm_s16le -fflags +bitexact -flags:a +bitexact -map_metadata -1 "$name.wav"
  rm -f "$tmp"
  printf '%-16s %5ss  peak %s dBFS\n' "$name.wav" "$dur" "$peak"
  if [[ "${CHECK:-}" == 1 ]]; then
    mkdir -p check
    ffmpeg -y -v error -i "$name.wav" -filter_complex \
      "[0:a]asplit[a][b];[a]showwavespic=s=900x200:colors=#ff4f8b[w];[b]showspectrumpic=s=900x300:legend=0:scale=log:fscale=log:stop=16000[s];[w][s]vstack" \
      -frames:v 1 "check/$name.png"
  fi
}

# ---------------------------------------------------------------------------------------------
# tongue-pop  THE signature. Three layers, 0.62 s:
#   1. pop:     sine swept 1470 -> 170 Hz with an 11 ms time constant, 28 ms decay (the cork).
#   2. droplet: 45 ms later, a rising chirp 500 -> 1700 Hz over 40 ms. A falling pop followed by
#               a rising blip is what makes it read as wet rather than as a kick drum.
#   3. boing:   a damped spring on pitch, the same shape as the film's tongue spring
#               (1 - e^-ap cos wp): f = 300 - 170 e^(-7.2y) cos(24.4y) Hz, so the note starts low,
#               overshoots, and settles as the tongue does. Phase is the closed-form integral.
#   Then a 17 ms slap echo for wetness.
gen tongue-pop -1 0.62 "
aevalsrc='0.95*(1-exp(-t*3000))*exp(-t/0.028)*sin($T*(170*t+1300*0.011*(1-exp(-t/0.011))))
 + gt(t,0.045)*0.55*exp(-max(t-0.045,0)/0.03)*sin($T*(500*(t-0.045)+15000*pow(t-0.045,2)))
 + gt(t,0.06)*0.5*(1-exp(-max(t-0.06,0)*250))*exp(-max(t-0.06,0)*7.5)*(
     sin($T*(300*(t-0.06)-170*(exp(-7.2*(t-0.06))*(-7.2*cos(24.4*(t-0.06))+24.4*sin(24.4*(t-0.06)))+7.2)/647.2))
     +0.3*sin(2*$T*(300*(t-0.06)-170*(exp(-7.2*(t-0.06))*(-7.2*cos(24.4*(t-0.06))+24.4*sin(24.4*(t-0.06)))+7.2)/647.2)))
 + 0.12*$N*exp(-t/0.004)':s=$SR:d=0.62,
highpass=f=70,aecho=0.9:0.7:17:0.22[o]"

# lick        The small second lick in the sting (3.3 s): droplet + a shorter, higher boing. 0.4 s.
gen lick -3 0.4 "
aevalsrc='0.6*exp(-t/0.03)*sin($T*(600*t+16000*t*t))
 + gt(t,0.03)*0.6*(1-exp(-max(t-0.03,0)*250))*exp(-max(t-0.03,0)*10)*
     sin($T*(420*(t-0.03)-150*(exp(-9*(t-0.03))*(-9*cos(31.4*(t-0.03))+31.4*sin(31.4*(t-0.03)))+9)/1067))':s=$SR:d=0.4,
highpass=f=120,aecho=0.9:0.6:15:0.2[o]"

# tick        Tape/trade print. 2.4 kHz sine with a 5 ms decay plus a 1 ms noise click. 40 ms.
gen tick -6 0.04 "
aevalsrc='exp(-t/0.005)*sin($T*2400*t)+0.5*$N*exp(-t/0.001)':s=$SR:d=0.04,highpass=f=800[o]"

# blip        UI blip for small pops (dot, diagram boxes, text lines). 1320 Hz + octave, pitch
#             nudges up 6 %, 25 ms decay. 110 ms.
gen blip -6 0.11 "
aevalsrc='(1-exp(-t*1500))*exp(-t/0.025)*(sin($T*1320*(t+0.3*t*t))+0.35*sin($T*2640*(t+0.3*t*t)))':s=$SR:d=0.11[o]"

# data-chirp  Order-book flicker: three 16 ms sine grains at 1.9 / 2.5 / 3.2 kHz, 20 ms apart.
#             Reads as 'data arrived', quieter and rounder than tick. 80 ms.
gen data-chirp -6 0.08 "
aevalsrc='exp(-mod(t,0.02)/0.004)*lt(t,0.06)*sin($T*(1900+650*floor(t/0.02))*mod(t,0.02))':s=$SR:d=0.08,
highpass=f=900,lowpass=f=7000[o]"

# draw        The wordmark drawing on (0.2 - 1.35 s): band-passed noise, like a marker on paper,
#             pulsing every 115 ms (the stagger between the six strokes of b-l-e-h), rising slightly in level. 1.15 s.
gen draw -9 1.15 "
aevalsrc='$N*(0.35+0.65*pow(sin(PI*t/0.115),2))*(0.5+0.5*t/1.15)*min(1,t/0.05)*min(1,(1.15-t)/0.2)':s=$SR:d=1.15,
bandpass=f=2600:t=q:w=1.1,highpass=f=900[o]"

# whoosh      Soft transition. Three noise bands (350 Hz, 1.1 kHz, 3.4 kHz), each with its own
#             bell envelope peaking at 30 % / 50 % / 68 % of the length, so the centre of energy
#             rises like a filter sweep. 0.6 s. whoosh-down is the same, reversed (falls).
WHOOSH="
aevalsrc='$N*pow(sin(PI*clip(t/0.36,0,1)),2)':s=$SR:d=0.6,bandpass=f=350:t=q:w=0.9[wa];
aevalsrc='$N*pow(sin(PI*clip((t-0.08)/0.44,0,1)),2)':s=$SR:d=0.6,bandpass=f=1100:t=q:w=0.9[wb];
aevalsrc='$N*pow(sin(PI*clip((t-0.2)/0.4,0,1)),2)':s=$SR:d=0.6,bandpass=f=3400:t=q:w=0.9[wc];
[wa][wb][wc]amix=inputs=3:normalize=0,highpass=f=150"
gen whoosh -6 0.6 "$WHOOSH[o]"
gen whoosh-down -6 0.6 "$WHOOSH,areverse[o]"

# slurp       The tongue wipe pulling back up: a sine glissando 180 -> 1000 Hz (exponential) with
#             a 34 Hz wet flutter, over a rising band of noise. 0.5 s.
gen slurp -5 0.5 "
aevalsrc='(0.6+0.4*sin($T*34*t))*sin(PI*clip(t/0.5,0,1))*sin($T*180*(exp(3.43*t)-1)/3.43)':s=$SR:d=0.5[sa];
aevalsrc='0.5*$N*pow(sin(PI*clip(t/0.5,0,1)),2)*(0.5+0.5*sin($T*34*t))':s=$SR:d=0.5,bandpass=f=2200:t=q:w=1.2[sb];
[sa][sb]amix=inputs=2:normalize=0,highpass=f=150[o]"

# stamp       Rubber stamp slamming down: sine thud dropping 150 -> 58 Hz (20 ms constant, 70 ms
#             decay), a 12 ms low-passed noise slap, and a 1 ms click. 0.28 s.
gen stamp -1 0.28 "
aevalsrc='exp(-t/0.07)*sin($T*(58*t+92*0.02*(1-exp(-t/0.02))))+0.25*exp(-t/0.03)*sin($T*233*t)':s=$SR:d=0.28[ta];
aevalsrc='$N*exp(-t/0.012)':s=$SR:d=0.28,lowpass=f=2600,highpass=f=300[tb];
[ta][tb]amix=inputs=2:weights='1 0.7':normalize=0[o]"

# signed      The pink tick landing on the signed block: a lighter thunk (110 -> 82 Hz) plus a
#             two-note bell, E6 then B6 30 ms later (a fifth: resolved, 'approved'). 0.5 s.
gen signed -3 0.5 "
aevalsrc='0.8*exp(-t/0.045)*sin($T*(82*t+60*0.015*(1-exp(-t/0.015))))
 + 0.45*exp(-t/0.12)*(sin($T*1318.5*t)+0.3*sin($T*2637*t))
 + gt(t,0.03)*0.4*exp(-max(t-0.03,0)/0.16)*(sin($T*1975.5*(t-0.03))+0.25*sin($T*3951*(t-0.03)))
 + 0.3*$N*exp(-t/0.003)':s=$SR:d=0.5[o]"

# sparkle     'Mirrored four ways': four pings 130 ms apart (the same stagger as the four mirror
#             boxes), E6 G6 B6 E7, each a sine with a 90 ms decay, then a shimmer from two short
#             echoes. 1.1 s.
gen sparkle -4 1.1 "
aevalsrc='0
 + gt(t,0.00)*exp(-max(t-0.00,0)/0.09)*sin($T*1318.5*(t-0.00))
 + gt(t,0.13)*exp(-max(t-0.13,0)/0.09)*sin($T*1568.0*(t-0.13))
 + gt(t,0.26)*exp(-max(t-0.26,0)/0.09)*sin($T*1975.5*(t-0.26))
 + gt(t,0.39)*exp(-max(t-0.39,0)/0.14)*(sin($T*2637.0*(t-0.39))+0.4*sin($T*3951*(t-0.39)))':s=$SR:d=1.1,
aecho=0.9:0.8:65|130:0.3|0.2[o]"

# deflate     Deadpan deflate for the problem scene, as the grey bars wipe over the prices:
#             a note sagging 392 -> 92 Hz (exponential), odd harmonics for a kazoo edge, vibrato
#             that widens as it dies, level sagging with the pitch, plus falling breath noise. 0.95 s.
gen deflate -3 0.95 "
aevalsrc='(1-exp(-t*200))*exp(-1.6*t)*min(1,(0.95-t)/0.25)*(
   sin($T*392*(1-exp(-1.53*t))/1.53+0.9*t*sin($T*6.5*t))
   +0.33*sin(3*($T*392*(1-exp(-1.53*t))/1.53+0.9*t*sin($T*6.5*t)))
   +0.2*sin(5*($T*392*(1-exp(-1.53*t))/1.53+0.9*t*sin($T*6.5*t))))':s=$SR:d=0.95,lowpass=f=2400[da];
aevalsrc='0.22*$N*exp(-t*2.5)':s=$SR:d=0.95,bandpass=f=1500:t=q:w=0.8[db];
[da][db]amix=inputs=2:normalize=0[o]"

# powerdown   The CDN mirror being crossed out: the same idea in 0.38 s, 330 -> 70 Hz, no vibrato.
gen powerdown -4 0.38 "
aevalsrc='(1-exp(-t*300))*min(1,(0.38-t)/0.12)*(
   sin($T*330*(1-exp(-4.1*t))/4.1)+0.33*sin(3*$T*330*(1-exp(-4.1*t))/4.1))':s=$SR:d=0.38,lowpass=f=2200[o]"

# shrug       'Nothing happens.' Two soft notes, G4 falling to E4 (the bleh motif), 110 ms each,
#             triangle-ish (odd harmonics rolled off). The three live mirrors bounce here. 0.34 s.
gen shrug -5 0.34 "
aevalsrc='lt(t,0.12)*(1-exp(-t*600))*exp(-t/0.07)*(sin($T*392*t)+0.11*sin($T*1176*t))
 + gt(t,0.12)*(1-exp(-max(t-0.12,0)*600))*exp(-max(t-0.12,0)/0.09)*(sin($T*329.63*(t-0.12))+0.11*sin($T*988.9*(t-0.12)))':s=$SR:d=0.34[o]"

# button      End-card hit: an E power chord (E3 B3 E4 + a quiet G#4 so it lands major and
#             friendly), 0.35 s decay, an 82 Hz thump underneath and a 2 ms click on top, with one
#             90 ms echo for a little room. 1.0 s.
gen button -1 1.0 "
aevalsrc='exp(-t/0.35)*(1-exp(-t*900))*(0.5*sin($T*164.81*t)+0.4*sin($T*246.94*t)+0.4*sin($T*329.63*t)+0.15*sin($T*415.3*t)+0.12*sin($T*659.26*t))
 + 0.9*exp(-t/0.06)*sin($T*(82.4*t+70*0.015*(1-exp(-t/0.015))))
 + 0.25*$N*exp(-t/0.002)':s=$SR:d=1.0,
aecho=0.9:0.5:90:0.18[o]"
