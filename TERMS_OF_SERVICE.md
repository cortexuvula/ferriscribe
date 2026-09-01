====================================================================
FERRISCRIBE TERMS OF SERVICE
Last updated: 28 August 2026
==============================================

These Terms of Service ("Terms") govern your use of FerriScribe, an
open source, local-first desktop application for recording clinical
encounters and generating SOAP note drafts. FerriScribe is developed
and maintained by the FerriScribe open source project ("the project").
Your use of FerriScribe means you accept these Terms. If you do not
accept, do not use the software.

These Terms are written in plain English so a practising physician can
read and understand them in roughly five minutes. Where we refer to
law, we cite the specific statute or standard.

--------------------------------------------------------------------
1.  WHAT FERRISCRIBE IS (AND ISN'T)
--------------------------------------------------------------------

1.1  FerriScribe is a SOFTWARE TOOL. It records audio from your
     clinical encounters, transcribes that recording, and produces a
     draft SOAP note. It does not make clinical decisions, it does not
     diagnose, and it is not a medical device.

1.2  You remain the treating physician. Every output from FerriScribe
     is a DRAFT that you must review, edit (if necessary), and
     approve before it becomes part of any patient record. The legal
     responsibility for the content of every medical note, referral
     letter, or other clinical document that enters a patient's chart
     is yours alone.

1.3  FerriScribe does not replace clinical judgment, professional
     accountability, or the obligations set out by the College of
     Physicians and Surgeons of British Columbia ("CPSBC"), including
     the CPSBC "Ethical Principles for Artificial Intelligence in
     Medicine."

--------------------------------------------------------------------
2.  HOW FERRISCRIBE WORKS: LOCAL AND OPTIONAL CLOUD
--------------------------------------------------------------------

2.1  Local-first by design. FerriScribe is architected so that, by
     default, audio recording and transcription happen on YOUR device
     (your computer). No audio, transcript, or note leaves your
     machine unless you choose to use a cloud model (see 2.2). This
     is a design property of the software, not a service promise.

2.2  Optional cloud models. Some transcription or language models may
     be made available through third-party cloud services. If you
     select a cloud model, the audio or text you submit may be
     transmitted to that third party solely for the purpose of
     generating a transcription or draft note. The project does not
     operate or control these third-party services; you should review
     their terms and privacy policies before enabling a cloud model.

2.3  You are responsible for choosing whether to use a local or cloud
     model and for ensuring that your choice is consistent with any
     applicable privacy policy of your practice, your College's
     guidance, and (if you work in a hospital or facility) your
     facility's policies.

--------------------------------------------------------------------
3.  YOUR OBLIGATIONS AS A USER (THE PHYSICIAN)
--------------------------------------------------------------------

3.1  Patient consent. Before recording any clinical encounter with
     FerriScribe, you must obtain and document the patient's consent
     to be recorded. The CMPA (Canadian Medical Protective Association,
     "AI Scribes: Answers to frequently asked questions," Dec 2023 /
     revised Dec 2025) and the CPSBC both state this requirement
     clearly. You should explain to the patient, in understandable
     language, that:

       (a)  an audio recording will be made of the encounter;
       (b)  the recording will be used by FerriScribe to draft a
            medical note;
       (c)  you will review and, if necessary, edit that draft before
            it enters the patient's record; and
       (d)  there are inherent privacy risks associated with digital
            processing, and AI may produce inaccurate or biased entries.

     You must document this consent discussion in the patient's
     medical record and keep any consent form if you use one.

3.2  Review and approval. You MUST review every AI-generated note,
     transcript segment, or suggestion produced by FerriScribe before
     it becomes part of a patient's medical record, EMR entry, or any
     other clinical document. You are responsible for correcting errors,
     omissions, or bias. The CMPA warns: "A patient who is injured
     because of an error in an unreviewed chart entry may launch a
     hospital complaint, College complaint, human rights complaint, or
     a legal action." FerriScribe does not review notes for you; the
     review obligation is yours.

3.3  EMR integration. If you import a FerriScribe draft into an
     electronic medical record ("EMR") system, you are responsible for
     ensuring that the import is accurate and that the resulting entry
     complies with your EMR vendor's requirements, any applicable
     record-keeping rules (including retention periods under the
     CPSBC and any applicable provincial regime), and your practice's
     privacy policy.

3.4  Facility authorization. If you are employed by or practising in
     a hospital, clinic, or other facility where you are NOT the
     custodian of patient information (as that term is used in BC's
     Personal Information Protection Act, SBC 2003, c 63 ("PIPA")),
     you must obtain facility or institutional authorisation before
     using FerriScribe.

3.5  Compliance with law and College guidance. You agree to use
     FerriScribe in compliance with applicable privacy legislation
     (including PIPA for custodians in British Columbia, and PIPEDA
     where federal law applies), the CPSBC Ethical Principles for AI
     in Medicine, and any guidance from your College or regulatory
     body. These Terms do not relieve you of any personal legal or
     professional obligation you have as a physician.

--------------------------------------------------------------------
4.  PRIVACY: WHAT THE SOFTWARE DOES (AND DOESN'T DO) WITH YOUR DATA
--------------------------------------------------------------------

4.1  Local-first architecture. FerriScribe is designed so that patient-
     identifiable audio, transcripts, and generated notes remain on
     YOUR device by default. The project does not collect, store, or
     process your patients' personal health information on remote
     servers, and the local-only operating mode involves no network
     transmission of patient data at all.

     When you enable an optional cloud model, data may be transmitted
     to a third-party provider. In that scenario:

       (a)  The project does not receive, store, or use your patients'
            personal health information for model training, product
            improvement, marketing, or any other secondary purpose.

       (b)  The project does not share your patients' personal health
            information with any party. Any data flow between your
            device and a cloud provider is initiated by you and
            governed by that provider's own terms and privacy policy.

4.2  No central data collection. The project does not operate a
     backend service that receives patient data. Crash reports,
     telemetry, and usage analytics (if any) are opt-in and contain
     no patient-identifiable information.

4.3  Breach notification. Because patient data resides on your device
     (and not on project-operated servers), breach notification
     obligations under PIPA, PIPEDA, or other privacy legislation
     generally fall on you as custodian. If the project becomes aware
     of a vulnerability in FerriScribe that could reasonably lead to
     a privacy breach, the project will endeavour to disclose it
     through the project's public channels (e.g., GitHub repository,
     project website) in a timely manner.

4.4  Data retention and deletion.

       (a)  Raw audio recordings, transcripts, and draft notes reside
            on YOUR device. It is your responsibility (not the
            project's) to manage retention and deletion of those files
            in accordance with your practice's policies, the CPSBC
            record-keeping guidance, and any applicable College or
            health authority policy.

       (b)  The CMPA notes that the Collège des médecins du Québec
            suggests raw recordings and verbatim transcripts of
            encounters qualify as "draft aids" that should be destroyed
            after the chart entry has been finalised. Other provinces'
            Colleges are generally silent on this point. We recommend
            you follow the guidance of your own College and any
            applicable health authority. FerriScribe does not
            automatically delete your local files; you control that
            process.

       (c)  If you use an optional cloud model and a third-party
            provider retains data on its servers, the retention period
            is governed by that provider's own terms.

4.5  Privacy impact assessments. Some jurisdictions (notably Québec
     and Alberta) require or encourage a Privacy Impact Assessment
     (PIA) before implementing an AI scribe. Because FerriScribe's
     default mode processes all data locally, the PIA analysis may
     differ from cloud-based scribe services. You are responsible for
     determining whether a PIA is required in your jurisdiction and,
     if so, completing one before use.

--------------------------------------------------------------------
5.  AI ACCURACY DISCLAIMER
--------------------------------------------------------------------

5.1  FerriScribe uses artificial intelligence ("AI") to produce
     transcriptions and draft clinical notes. AI systems may:

       (a)  "hallucinate" -- produce text that sounds plausible but is
            factually incorrect;
       (b)  misinterpret audio, especially in the presence of strong
            accents, overlapping speech, background noise, or
            technical audio issues;
       (c)  introduce bias -- reproduce patterns present in the training
            data that may not be appropriate for a particular patient
            or clinical context; and
       (d)  omit information -- fail to capture details from the
            encounter.

5.2  FerriScribe is NOT a medical device under the Canadian
     Medical Devices Regulations (SOR/98-282). It is a drafting aid.
     The project does not represent or warrant that any output from
     FerriScribe is clinically accurate, complete, or appropriate for
     any particular patient. The output must be reviewed and approved
     by a qualified physician before use in any clinical context.

--------------------------------------------------------------------
6.  YOUR RESPONSIBILITY FOR CLINICAL ACCURACY
--------------------------------------------------------------------

This section follows the CMPA's guidance (Dec 2025 revision). The CMPA
specifically warns against "blanket clauses that shift all risk to the
physician." Under these Terms, you retain full ownership of clinical
judgment, and the project makes no claim to it.

6.1  You are responsible for:

       (a)  obtaining and documenting patient consent;
       (b)  reviewing, editing, and approving every AI-generated note
            or draft before it enters a patient record;
       (c)  the clinical accuracy, completeness, and appropriateness of
            every medical note, referral letter, or other clinical
            document that enters a patient's record, regardless of
            whether FerriScribe contributed to its drafting;
       (d)  compliance with your College's guidance, privacy legislation
            (PIPA, PIPEDA, or any applicable provincial regime), and any
            facility or institutional policy;
       (e)  EMR import accuracy and record-keeping compliance,
            including retention periods; and
       (f)  any loss or damage that results from your failure to perform
            the obligations in this Section 6.1.

6.2  The project does not and cannot accept responsibility for clinical
     outcomes. FerriScribe is a drafting tool operated by you on your
     own device. The project does not review, approve, or guarantee
     the accuracy of any note you produce with the software.

--------------------------------------------------------------------
7.  OPEN SOURCE DISCLAIMER AND LIABILITY
--------------------------------------------------------------------

7.1  FerriScribe is open source software made available freely under
     the terms of its open source licence (see Section 9). Consistent
     with open source norms, the software is provided "AS IS" and "AS
     AVAILABLE," without warranty of any kind, express or implied.

7.2  To the maximum extent permitted by applicable law, the project
     and its contributors disclaim all warranties, including (but not
     limited to) implied warranties of merchantability, fitness for a
     particular purpose, non-infringement, and any warranty that the
     software will be error-free, uninterrupted, or secure.

7.3  To the maximum extent permitted by applicable law, in no event
     shall the project or its contributors be liable for any claim,
     damages, or other liability, whether in contract, tort (including
     negligence), breach of statutory duty, or otherwise, arising from
     or in connection with your use of FerriScribe. This includes,
     without limitation, any direct, indirect, incidental, special,
     consequential, or punitive damages, including (but not limited
     to) loss of profits, loss of data, loss of goodwill, business
     interruption, personal injury, or death.

7.4  Nothing in this Section 7 excludes or limits liability for:

       (a)  fraud or fraudulent misrepresentation by the project; or
       (b)  any liability that cannot, as a matter of British Columbia
            law, be excluded or limited.

7.5  The CMPA recommends that AI scribe providers accept
     responsibility for their own privacy or security failures. Because
     FerriScribe's default architecture processes all patient data
     locally on your device, the project does not have possession or
     control of your patient data and cannot be responsible for a
     privacy or security breach of data on your own systems. Where
     liability could arise from the project's own conduct (for
     example, if the project knowingly distributed malicious code),
     the project accepts responsibility for the consequences of that
     conduct, subject to the limitations in this Section 7.

--------------------------------------------------------------------
8.  INDEMNIFICATION
--------------------------------------------------------------------

8.1  You agree to indemnify and hold harmless the project and its
     contributors from any claim, demand, loss, damage, cost, or
     expense (including reasonable legal fees) arising out of or
     related to:

       (a)  your use of FerriScribe, including any clinical note or
            document you produce with FerriScribe and place in a
            patient record;
       (b)  your failure to obtain or document patient consent as
            required by Section 3.1;
       (c)  your failure to review and approve AI-generated content as
            required by Section 3.2;
       (d)  your breach of these Terms or of any applicable law, College
            rule, or facility policy; and
       (e)  your gross negligence or wilful misconduct in connection with
            FerriScribe.

8.2  This indemnification is specific, not blanket. It covers claims
     that arise from YOUR conduct, not from the project's conduct. It
     does not require you to indemnify the project for loss or damage
     caused by the project's own actions. This is consistent with the
     CMPA's guidance that contracts should avoid "blanket clauses that
     shift all risk to the physician."

--------------------------------------------------------------------
9.  INTELLECTUAL PROPERTY
--------------------------------------------------------------------

9.1  FerriScribe and its source code are made available under an open
     source licence on GitHub (cortexuvula/ferriscribe). The open
     source licence governs your right to copy, modify, and
     redistribute the source code.

9.2  These Terms of Service govern your USE of the software as a tool
     in clinical practice and are separate from, and additional to,
     the open source licence. If the terms of the open source licence
     and these Terms conflict, the open source licence governs your
     rights to the source code; these Terms govern your use of
     FerriScribe in connection with patient encounters and clinical
     documentation.

9.3  You retain all right, title, and interest in and to the patient
     data (audio recordings, transcripts, draft notes, and any
     edited or approved final notes) that you generate using
     FerriScribe. The project claims no ownership in your patient data.

--------------------------------------------------------------------
10.  TERM AND TERMINATION
--------------------------------------------------------------------

10.1 These Terms take effect on the date you first install or use
     FerriScribe and continue until terminated.

10.2 You may terminate these Terms at any time by ceasing to use
     FerriScribe.

10.3 On termination, your obligations under Sections 3 (Your
     Obligations), 6 (Your Responsibility for Clinical Accuracy),
     8 (Indemnification), and 12 (Governing Law) survive termination
     to the extent necessary to give them effect.

--------------------------------------------------------------------
11.  GOVERNING LAW AND JURISDICTION
--------------------------------------------------------------------

11.1 These Terms are governed by and construed in accordance with the
     laws of the Province of British Columbia and the applicable laws
     of Canada (federal) that apply within British Columbia.

11.2 The courts of British Columbia have exclusive jurisdiction to
     hear and determine any dispute arising out of or in connection
     with these Terms, and you irrevocably submit to that jurisdiction.

11.3 The Limitations Act, RSBC 2012, c 165 (the "Limitations Act")
     applies. Subject to any longer period that cannot be contracted
     out of, any action or proceeding arising out of these Terms or
     the use of FerriScribe must be commenced within the basic
     limitation period set out in s. 6 of the Limitations Act (two
     years from the date the claim was discovered) or any extended
     period that may apply.

--------------------------------------------------------------------
12.  PROVINCIAL VARIATION
--------------------------------------------------------------------

12.1 These Terms are grounded in British Columbia law (PIPA, CPSBC
     standards, the Limitations Act, and BC common law). FerriScribe
     may be used by physicians in other Canadian provinces or
     territories. Where a physician's province has different privacy
     legislation (for example, PIPEDA in a non-PIPA province, or
     Quebec's Act respecting the protection of personal information in
     the private sector), different College guidance, or different
     record-keeping rules, that physician is responsible for complying
     with the applicable provincial requirements in addition to these
     Terms. These Terms do not purport to override or displace any
     more protective provincial obligation that applies to you.

--------------------------------------------------------------------
13.  MISCELLANEOUS
--------------------------------------------------------------------

13.1 Entire agreement. These Terms constitute the entire agreement
     between you and the FerriScribe project relating to your use of
     FerriScribe and supersede any prior understandings or agreements
     on that subject.

13.2 Amendment. These Terms may be amended from time to time. If a
     material amendment is made, the project will endeavour to notify
     users (for example, through the project's GitHub repository or
     website). Your continued use of FerriScribe after the amendment
     date means you accept the amended Terms.

13.3 Severability. If any provision of these Terms is found to be
     invalid or unenforceable by a court of competent jurisdiction, the
     remaining provisions remain in full force and effect.

13.4 Waiver. No failure or delay in exercising any right or remedy
     under these Terms operates as a waiver of that right or remedy.

13.5 Third-party tools. FerriScribe may incorporate or integrate with
     third-party software, libraries, or cloud services. Your use of
     those third-party components is subject to their own terms and
     privacy policies. The project is not a party to any agreement
     between you and a third-party component provider.

13.6 No partnership or agency. Your use of FerriScribe does not create
     a partnership, joint venture, employment, or agency relationship
     between you and the project or any contributor.

--------------------------------------------------------------------
14.  CONTACT
--------------------------------------------------------------------

Questions about these Terms may be directed to the FerriScribe project
through its public channels:

    GitHub: https://github.com/cortexuvula/ferriscribe

====================================================================
TERMS OF SERVICE -- FERRISCRIBE
=============================================
