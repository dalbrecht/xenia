// Camera frames remain on this device. The decoder is loaded on demand.
let pairScanGeneration = 0, pairScanStream = null, pairScanTimer = null, pairDecoderPromise;
function stopPairScan() {
  pairScanGeneration++;
  clearTimeout(pairScanTimer);
  pairScanStream?.getTracks().forEach(track => track.stop());
  pairScanStream = null;
  const video = document.querySelector('#scan-video');
  video.pause(); video.srcObject = null;
  document.querySelector('#scan-preview').hidden = true;
  document.querySelector('#scan-pair').disabled = false;
}
function loadPairDecoder() {
  if (window.jsQR) return Promise.resolve(window.jsQR);
  if (!pairDecoderPromise) pairDecoderPromise = new Promise((resolve, reject) => {
    const script = document.createElement('script');
    script.src = '/vendor/jsQR-1.4.0.js';
    script.onload = () => resolve(window.jsQR);
    script.onerror = () => { script.remove(); pairDecoderPromise = null; reject(Error('Could not load the QR scanner. Reconnect and try again, or use a pairing link.')); };
    document.head.append(script);
  });
  return pairDecoderPromise;
}
async function startPairScan(onCode, onError) {
  stopPairScan();
  const generation = pairScanGeneration;
  const preview = document.querySelector('#scan-preview'), video = document.querySelector('#scan-video'), status = document.querySelector('#scan-status');
  document.querySelector('#scan-pair').disabled = true;
  preview.hidden = false; status.textContent = 'Allow camera access, then point it at the controller’s QR code.';
  try {
    if (!navigator.mediaDevices?.getUserMedia) throw Error('Camera scanning requires HTTPS and camera support. Use a pairing link instead.');
    // Request from the tap handler before awaiting the decoder, for iOS.
    const camera = navigator.mediaDevices.getUserMedia({audio:false, video:{facingMode:{ideal:'environment'},width:{ideal:640},height:{ideal:480}}}).then(stream => {
      if (generation !== pairScanGeneration) stream.getTracks().forEach(track => track.stop());
      else pairScanStream = stream;
      return stream;
    });
    const decoderPromise = loadPairDecoder();
    // Observe both promises immediately; permission rejection must not leave a
    // decoder rejection unhandled, and a late stream must always be stopped.
    const [streamResult, decoderResult] = await Promise.allSettled([camera, decoderPromise]);
    if (streamResult.status === 'fulfilled') {
      if (generation !== pairScanGeneration || decoderResult.status === 'rejected') streamResult.value.getTracks().forEach(track => track.stop());
      else pairScanStream = streamResult.value;
    }
    if (generation !== pairScanGeneration) return;
    if (streamResult.status === 'rejected') throw Error('Camera unavailable or permission denied. Allow camera access and retry, or use a pairing link.');
    if (decoderResult.status === 'rejected') throw decoderResult.reason;
    video.srcObject = pairScanStream; await video.play();
    if (generation !== pairScanGeneration) return;
    const canvas = document.createElement('canvas'), context = canvas.getContext('2d', {willReadFrequently:true});
    const decode = decoderResult.value;
    status.textContent = 'Point the camera at a fresh Xenia pairing QR code.';
    const tick = () => {
      if (generation !== pairScanGeneration) return;
      try {
        if (video.readyState >= 2 && video.videoWidth) {
          const ratio = Math.min(1, 640 / video.videoWidth);
          canvas.width = Math.round(video.videoWidth * ratio); canvas.height = Math.round(video.videoHeight * ratio);
          context.drawImage(video, 0, 0, canvas.width, canvas.height);
          const frame = context.getImageData(0, 0, canvas.width, canvas.height);
          const code = decode(frame.data, frame.width, frame.height, {inversionAttempts:'dontInvert'});
          if (code) { stopPairScan(); Promise.resolve(onCode(code.data)).catch(onError); return; }
        }
        pairScanTimer = setTimeout(tick, 250);
      } catch(error) { stopPairScan(); onError(error); }
    };
    tick();
  } catch(error) { if(generation === pairScanGeneration) {stopPairScan(); onError(error);} }
}
document.addEventListener('visibilitychange', () => {if(document.hidden) stopPairScan();});
window.addEventListener('pagehide', stopPairScan);
