# HRTF data licences

BEAM places sounds around you with head-related transfer function (HRTF) measurements made by other people. You choose between the sets below with **HRTF Dataset**, in the configurator's Compass Clicks group. Each set ships as an extract of its measurement rings between 20° below and 10° above ear level (`hrtf_*.npz`), baked from the original SOFA file by `bake_hrtf.py` and stored as measured. When BEAM loads a set it processes it: it separates each response's interaural time difference from its frequency response, interpolates to the chosen elevation and to every whole degree of azimuth, makes left and right mirror images, optionally equalises the tone colour common to every direction, rebuilds the responses as minimum-phase filters, and matches the level to the MIT KEMAR set.

## MIT KEMAR — `hrtf_mit_kemar.npz`

- **Source:** MIT Media Lab, KEMAR dummy head, normal pinna. SOFA conversion by Piotr Majdak and Michael Mihocic, Acoustics Research Institute, Austrian Academy of Sciences.
- **Licence:** "This HRTF data is provided without any usage restrictions. It is requested that you cite Gardner and Martin (1995) when using this data for research or commercial applications."
- **Citation:** Gardner, W. G., and Martin, K. D. (1995). "HRTF measurements of a KEMAR." *J. Acoust. Soc. Am.* 97, 3907–3908.
- **Changes:** extract of the rings from −20° to +10°, processed at load as described above.
- **Original:** https://sofacoustics.org/data/database/mit/mit_kemar_normal_pinna.sofa

## Neumann KU100 (TH Köln) — `hrtf_thk_ku100.npz`

- **Source:** Benjamin Bernschütz, Technische Hochschule Köln. Neumann KU100 dummy head, full sphere on a 2° grid (`HRIR_FULL2DEG`).
- **Licence:** Creative Commons Attribution-ShareAlike 3.0 (CC BY-SA 3.0), as stated in the SOFA file. The Zenodo record lists CC BY 3.0; BEAM follows the stricter of the two. https://creativecommons.org/licenses/by-sa/3.0/
- **Changes:** extract of the rings from −20° to +10°, processed at load as described above. The extract (`hrtf_thk_ku100.npz`) is distributed under the same CC BY-SA 3.0 licence.
- **Citation:** Bernschütz, B. (2013). "A Spherical Far Field HRIR/HRTF Compilation of the Neumann KU 100." *Proceedings of the 40th Italian (AIA) Annual Conference on Acoustics and the 39th German Annual Conference on Acoustics (DAGA)*, 592–595.
- **Original:** https://zenodo.org/records/3928297

## KEMAR (SADIE II, University of York) — `hrtf_sadie_kemar.npz`

- **Source:** SADIE II database, subject D2 (KEMAR), 48 kHz. AudioLab, Department of Electronic Engineering, University of York. Diffuse-field equalised by its authors.
- **Licence:** Copyright 2018, University of York. Licensed under the Apache License, Version 2.0. https://www.apache.org/licenses/LICENSE-2.0
- **Changes:** extract of the rings from −25° to +15°, processed at load as described above.
- **Citation:** Armstrong, C., Thresh, L., Murphy, D., and Kearney, G. (2018). "A Perceptual Evaluation of Individual and Non-Individual HRTFs: A Case Study of the SADIE II Database." *Applied Sciences* 8(11), 2029. https://doi.org/10.3390/app8112029
- **Original:** https://zenodo.org/records/12092466
