# The lawfulness of an independent, read-only interoperability client under EU and French law

*A documented legal basis. Not legal advice — see the disclaimer at the end.*

This repository hosts independent, unofficial clients ("extensions") that let a person bring their
own data out of an online service into software they control. This document sets out, under European
Union and French law, the legal basis on which such a client is developed and used: read-only, with
the account holder's own credentials and consent, reaching only their own data, and never touching
any other account or circumventing any security measure.

## Plain-language summary

European Union law gives the person who lawfully uses a piece of software the right to
study how it works and, where necessary for interoperability, to analyse and reconstruct the
information needed to make an independent program work with it. These rights are deliberately
placed beyond the reach of contract: under the Software Directive, any licence term or terms-of-use
clause that purports to forbid the core interoperability acts is, in the words of the statute,
**"null and void"** (Directive 2009/24/EC, Article 8; transposed in France at Article L122-6-1
of the Code de la propriété intellectuelle, whose closing sentence makes contrary stipulations
*"nulle et non avenue"*). The Court of Justice has held that a program's functionality, its
programming language and its data-file formats are not, as such, protected by copyright, so
observing and reproducing them to achieve interoperability does not infringe.

Applied to this project — an independent client that lets the holder of a professional account,
**using their own credentials and with their own consent, read their own agenda and client
records in read-only form** so the data can be displayed elsewhere, checked for double-bookings,
and planned across tools — the legal footing is solid. The client never touches any other person's
account, never circumvents anyone else's authentication, never scrapes the public booking graph,
and never redistributes the platform's code.

**The one honest caveat.** None of this is a blanket immunity. The interoperability *acts on the
software* cannot be contracted away, but the Court of Justice held in *Ryanair v PR Aviation*
(C-30/14) that where the underlying *data compilation* is protected by neither copyright nor the
database sui generis right, its maker may still restrict use *by contract*. And the principal
practical risk is not copyright at all but the French computer-intrusion offence (Code pénal,
Articles 323-1 et seq.): that offence turns on *fraudulent* access — access one is not authorised
to have, or obtained by defeating a security measure. Accessing one's own account with one's own
valid credentials is authorised access, which is exactly why the project's scope keeps it on the
lawful side of that line.

---

## 1. The EU framework

### 1.1 Software Directive 2009/24/EC — the right to study, and the right to interoperate

Directive 2009/24/EC of 23 April 2009 on the legal protection of computer programs (the codified
successor to Directive 91/250/EEC) protects a program's *expression* by copyright, but it carves
out mandatory exceptions for the lawful user.

**Article 5(3)** — observe, study, test:

> "The person having a right to use a copy of a computer program shall be entitled, without the
> authorisation of the rightholder, to observe, study or test the functioning of the program in
> order to determine the ideas and principles which underlie any element of the program if he does
> so while performing any of the acts of loading, displaying, running, transmitting or storing the
> program which he is entitled to do."

This is black-letter authorisation for dynamic analysis: watching what a program sends and receives,
in order to work out the ideas behind it, is a right, not a tolerated act — provided one is entitled
to run the program in the first place.

**Article 6** — decompilation for interoperability. Where going further (reproducing or translating
code) is *"indispensable to obtain the information necessary to achieve the interoperability of an
independently created computer program with other programs"*, Article 6(1) removes the need for the
rightholder's authorisation, subject to three conditions: the acts are performed by (or for) a person
with a right to use the program; the interoperability information *"has not previously been readily
available"* to that person; and the acts are *"confined to the parts of the original program which
are necessary in order to achieve interoperability."* Article 6(2) limits what may be done with the
information so obtained — in particular it may not be used *"for the development, production or
marketing of a computer program substantially similar in its expression"*, nor given to others except
where necessary for interoperability. Article 6(3) adds the Berne-style safeguard that the Article
may not be read to prejudice the rightholder's *"legitimate interests"* or the *"normal exploitation"*
of the program.

**Article 8 — the cornerstone.** The two rights above cannot be waived by contract:

> "Any contractual provisions contrary to Article 6 or to the exceptions provided for in Article
> 5(2) and (3) shall be null and void."

So a licence or a set of terms of use that purports to prohibit observing, studying and testing the
program (Art. 5(3)), making a back-up (Art. 5(2)), or decompiling for interoperability (Art. 6) is
inoperative to that extent. Recital 15 frames the policy: the exceptions exist *"to make it possible
to connect all components of a computer system"* and *"to achieve the interoperability of an
independently created program with other programs"*; recital 10 defines interoperability as *"the
ability to exchange information and mutually to use the information which has been exchanged"*; and
recital 16 states the rationale for voiding contrary decompilation clauses.

A practical point for this project: much of the work here does **not** even require Article 6.
Reading one's own agenda through the platform's private web/API, by observing the traffic the account
already produces, is the Article 5(3) situation — observing and testing the functioning of a service
one is entitled to run. Article 6's stricter decompilation conditions matter only if one needs to
translate the platform's own program code, which this project does not do.

### 1.2 Database Directive 96/9/EC — the maker's right, and the lawful user

Directive 96/9/EC gives the maker of a database that represents a *"substantial investment"* in
obtaining, verifying or presenting its contents a *sui generis* right (Article 7) to prevent
*"extraction and/or re-utilization of the whole or of a substantial part"* of the contents. But
Article 8(1) protects the lawful user:

> "The maker of a database which is made available to the public in whatever manner may not prevent a
> lawful user of the database from extracting and/or re-utilizing insubstantial parts of its contents
> ... for any purposes whatsoever."

Two distinctions matter for an account holder reading their own records:

- **Whose investment?** The sui generis right rewards the database maker's investment in *obtaining,
  verifying or presenting* contents. The appointments and client records at issue here were created
  and entered by the account holder's own business. Data that the claimant merely *created* (as opposed
  to collecting pre-existing data) sits at the weak edge of sui generis protection — the CJEU's
  "spin-off" line of authority (e.g. *British Horseracing Board*, C-203/02, and the *Fixtures Marketing*
  judgments, both of 9 November 2004) distinguishes investment in *creating* data from investment in
  *collecting* it. A platform's
  claim to a sui generis right over records its customers typed in is therefore far from automatic.
- **Insubstantial parts.** Even where a protected database exists, Article 8(1) guarantees the lawful
  user access to insubstantial parts for any purpose, and that guarantee cannot be contracted away
  *when the Directive applies* (Article 15 makes Articles 8 and 6(1) mandatory). The honest limit on
  this is set out in §4 below (*Ryanair*).

### 1.3 Trade Secrets Directive (EU) 2016/943 — reverse engineering is a lawful means

Article 3(1)(b) lists, among the ways a trade secret may be *lawfully* acquired:

> "observation, study, disassembly or testing of a product or object that has been made available to
> the public or that is lawfully in the possession of the acquirer of the information who is free from
> any legally valid duty to limit the acquisition of the trade secret".

Reverse engineering a product one lawfully holds is thus expressly a lawful means of acquiring the
know-how inside it. The clause is subject to any *"legally valid duty to limit"* acquisition — i.e.
a confidentiality obligation — but this is where the Software Directive's Article 8 bites back: a
contractual term that forbids the Article 5(3)/Article 6 interoperability acts is void, and a void
term cannot be the *"legally valid duty"* that defeats the trade-secrets safe harbour for those acts.

### 1.4 Data Act (Regulation (EU) 2023/2854) — access and anti-lock-in, with scope uncertainty

The Data Act, applicable from **12 September 2025** (several provisions, notably those on switching
between data-processing/cloud services, phased in to 2026-2027), pursues user access to the data they
generate and the dismantling of lock-in. Its relevance here is **genuinely uncertain and should not
be overstated**:

- Its "connected product" / "related service" access regime is built around physical products that
  generate data through use (IoT devices) and the services attached to them. A browser-based salon
  booking SaaS is not obviously a "connected product".
- Whether such a platform is a "data processing service" (the category carrying the switching and
  portability obligations, aimed principally at cloud/edge infrastructure) is arguable but far from
  settled.

The candid position is that the Data Act expresses a clear EU policy *direction* toward data access
and interoperability that is favourable to this project's aims, but that no specific Data Act
obligation can be asserted as a settled legal basis for it today.

### 1.5 GDPR (Regulation (EU) 2016/679) — read the client-list angle honestly

The client list deserves a careful, non-overstated treatment, because the data portability right is
frequently mis-cited here.

- **Article 20 (portability)** gives *the data subject* the right to receive *"the personal data
  concerning him or her"* that they provided to a controller *"in a structured, commonly used and
  machine-readable format"*, and to transmit it, including *"directly from one controller to another,
  where technically feasible."* The salon's clients are the data subjects; the portability right is
  *theirs*, exercised against their controller. It is **not** a right the salon can invoke to pull its
  client list out of the platform.
- The correct GDPR framing is different and stronger: in respect of its own client records, the salon
  is the **data controller**, and the booking platform processes those records **on the salon's behalf
  as a processor** (GDPR Article 28). A controller directing and retrieving the processing of records
  it is responsible for is acting squarely within its own role; a processor is in principle obliged to
  make the controller's data available to it and to assist it. Reading one's own client records that a
  processor holds for one is therefore an ordinary incident of being the controller — not an intrusion
  into someone else's data.
- Article 15 (right of access) reinforces the general principle that personal data should be accessible
  to those entitled to it, but again it is the *data subject's* right, not the controller's.

Net: the client-list interoperability is best grounded in the salon's lawful possession and control
of its own client records (as controller), not in a portability claim — and any onward use of clients'
personal data remains subject to the salon's own GDPR obligations as controller.

---

## 2. The French framework

### 2.1 Code de la propriété intellectuelle — Article L122-6-1

France transposed the Software Directive at Article L122-6-1 CPI (exceptions to the author's
restricted acts defined in Article L122-6). Its structure mirrors the Directive:

- **III** — observation, étude, test: the lawful user *"peut sans l'autorisation de l'auteur observer,
  étudier ou tester le fonctionnement de ce logiciel afin de déterminer les idées et principes qui
  sont à la base de n'importe quel élément du logiciel"*. This is the French Article 5(3).
- **IV** — décompilation pour l'interopérabilité: reproduction or translation of the code is permitted
  where *"indispensable pour obtenir les informations nécessaires à l'interopérabilité"*, under the
  same three conditions and the same use-limits as Directive Article 6.
- **V** — the general safeguard against prejudicing *"l'exploitation normale du logiciel"* or the
  author's *"intérêts légitimes"*.

And the decisive closing sentence:

> "Toute stipulation contraire aux dispositions prévues aux II, III et IV du présent article est nulle
> et non avenue."

Note the exact scope: the nullity covers paragraph **II** (back-up copy), **III** (observe/study/test)
and **IV** (decompilation for interoperability). Paragraph I (acts necessary to *use* the software) is
treated differently — there the author may contractually reserve the right to correct errors. For this
project, the operative rights are III and IV, both within the non-waivable set.

French case law has consistently upheld the interoperability/decompilation right as a matter of public
order; the Conseil d'État has reaffirmed that a licence cannot neutralise it. (The French transposition
of the database sui generis right sits separately at CPI Articles L341-1 et seq., with the lawful-user
rule at L342-3.)

### 2.2 Code pénal — Articles 323-1 et seq. (the STAD offences, "loi Godfrain")

This is the **principal legal risk** and deserves the plainest statement. Article 323-1, first paragraph:

> "Le fait d'accéder ou de se maintenir, frauduleusement, dans tout ou partie d'un système de
> traitement automatisé de données est puni de trois ans d'emprisonnement et de 100 000 € d'amende."

(Aggravated to five years / €150,000 where data is deleted or altered or the system's functioning is
impaired, and to seven years / €300,000 where the system is a State personal-data system.) Article
323-2 punishes impairing a system; Article 323-3 punishes fraudulently introducing, extracting, holding,
reproducing, transmitting, deleting or modifying its data.

The pivotal word is **"frauduleusement"**. The offence is not committed by accessing a system; it is
committed by accessing or remaining in it *fraudulently* — that is, without being authorised, or against
a security measure, with awareness that one is not entitled to be there. **Accessing one's own account,
with one's own valid credentials, is authorised access and is not "frauduleux."**

The leading cautionary authority is the *Bluetouff* case (Cass. crim., 20 May 2015, n° 14-81336). A
journalist who reached files on a public-facing server was ultimately convicted — but the conviction
rested on *maintien frauduleux* (continuing to browse and download *after he had realised* the data was
not meant to be openly accessible) and on theft of the files, **not** on fraudulent *access* in the first
instance. The lesson that matters here is the inverse of the project's situation: *Bluetouff* turned on a
person using data they knew they were not entitled to. A user reading their **own** account's data, which
they are plainly entitled to, with credentials issued to them, is on the opposite side of that line.

### 2.3 Breach of terms of service is a civil matter, not a crime

In France and in the EU generally, breaching a contract's terms of use is a civil contractual matter —
it may expose the party to damages or termination, but it is not, in itself, a criminal offence. French
law does not convert a terms-of-use violation into fraudulent access under Article 323-1: the criminal
offence requires fraudulent access to or maintenance in the *system*, not mere non-compliance with usage
conditions by someone who is entitled to be in the system.

This contrasts with the position in the United States, offered **for context only and with no bearing on
EU or French law**: in *Van Buren v. United States* (2021) the US Supreme Court read the Computer Fraud
and Abuse Act's "exceeds authorized access" narrowly, on a "gates-up-or-down" basis — a person authorised
to access a system does not commit the federal offence merely by using that access for a purpose the
terms forbid. The Court expressly left open (footnote 8) whether the test turns on code-based barriers
alone or also on contractual limits. The comparison is illustrative of a broad common direction, not a
source of EU law.

---

## 3. The case law of the Court of Justice

- **SAS Institute v World Programming, C-406/10 (2 May 2012).** The CJEU held that neither the
  functionality of a computer program, nor its programming language, nor the format of its data files
  constitutes, *as such*, a form of expression protected by copyright under the Software Directive, and
  that a lawful user may observe, study and test the program to determine the underlying ideas without
  the rightholder's authorisation. This is the decisive authority that interface behaviour and data
  formats may be studied and reproduced at the level of ideas. (The Court also noted that the Information
  Society Directive might protect a language or format if it were itself the author's own intellectual
  creation, and that copying the program's *own* expression would still infringe.)

- **Sony v Datel, C-159/23 (17 October 2024).** The Court confined the protected *expression* of a
  computer program to its code (source and object code). The content of variable data that a program
  writes into RAM and uses while running is not protected, because that content does not allow the
  program itself to be reproduced or recreated. The significance for interoperability is that reading
  and reacting to the *data* a service holds or exchanges — as opposed to copying its program code — is
  outside the scope of software copyright.

- **UsedSoft v Oracle, C-128/11 (3 July 2012).** Principally on exhaustion of the distribution right —
  including for software delivered by download — and the concept of the "lawful acquirer". It supports
  the broader principle that a person who has lawfully acquired the right to use software enjoys the
  user rights the Directive attaches to that status, and that certain of those positions cannot be
  defeated by contract.

- **Nintendo v PC Box, C-355/12 (23 January 2014).** On technological protection measures (TPMs): they
  are protected only in so far as they serve to prevent acts that infringe copyright, and a national
  court must assess the **proportionality** of a TPM and whether a circumventing device has commercially
  significant purposes other than infringement. The relevance here is at the level of principle — the law
  does not grant TPMs an unlimited veto over legitimate interoperability — but it is also a caution: this
  project deliberately does **not** circumvent any technological protection or authentication measure, so
  it does not put itself in the TPM debate at all.

- **Ryanair v PR Aviation, C-30/14 (15 January 2015) — the honest limit.** The Court held that where a
  database is protected by *neither* copyright *nor* the sui generis right, the Database Directive's
  mandatory lawful-user provisions (Articles 6(1), 8 and 15) simply do not apply, so the Directive does
  **not** preclude the maker from restricting use of that database **by contract**. In other words, the
  "terms of use cannot bind me" argument has a real boundary: it is strong for the acts the Software
  Directive protects (which Article 8 of that Directive renders non-waivable), but it does **not**
  guarantee that a platform's contractual limits on *use of the data* are unenforceable where the data
  compilation falls outside both copyright and the sui generis right. This nuance is addressed head-on in
  the risks section.

---

## 4. Risks and limits (stated candidly)

1. **The STAD offence is the real risk, and scope is the answer to it.** The one way this kind of project
   crosses into criminal territory is by accessing a system *fraudulently* — using someone else's account,
   defeating an authentication or security measure, or accessing areas one is not entitled to. The project
   is designed to do none of these. It uses the account holder's own credentials, with their consent, to
   read their own account only. That is authorised access. The margin of safety is preserved only so long
   as the implementation never circumvents authentication, never accesses another account, and never
   evades a security control.

2. **The *Ryanair* contract nuance.** For the *acts on the software* — observing, studying, testing, and,
   if ever needed, decompiling for interoperability — contrary terms are void (Software Directive Art. 8;
   CPI L122-6-1). But for *use of the extracted data*, if the agenda/client compilation is protected by
   neither copyright nor the sui generis right, *Ryanair* means the platform may lawfully impose contractual
   use restrictions, and breach of those would be a civil contract matter. This memo does not claim that
   every term of service is unenforceable. It claims the narrower and well-founded point that the core
   interoperability acts cannot be contracted away, and that reading one's own records for one's own
   interoperability is a weak target for a sui generis claim in any event.

3. **Data Act scope is uncertain.** The Data Act points firmly in this project's favour as a matter of EU
   policy, but it cannot be asserted as a concrete legal obligation binding this particular platform until
   its scope (connected product / related service / data processing service) is clarified for SaaS of this
   kind. Do not lean on it as a primary basis.

4. **GDPR cuts both ways.** The salon's control over its own client records supports access; but the salon
   remains the controller of its clients' personal data and must honour its own GDPR duties (purpose
   limitation, security, the clients' own rights) in any onward processing.

5. **This is not an immunity.** It is a reasoned legal basis assembled from public primary sources. Whether
   a given platform's particular terms, security measures and data are such that a given act is lawful is
   fact-dependent and, ultimately, for a court.

---

## 5. The scope that keeps this lawful

The design choices below are not incidental; each one maps to a legal condition above and is what keeps
the project inside the lawful zone.

- **The account holder's own credentials and explicit consent.** This makes the access *authorised* — the
  answer to Code pénal 323-1 — and makes the person a "lawful user" for Software Directive Articles 5(3)
  and 6 and Database Directive Article 8.
- **Read-only.** No data is introduced, altered, deleted or impaired, so Articles 323-1 (aggravated),
  323-2 and 323-3 of the Code pénal are not engaged; nothing conflicts with the normal exploitation of the
  service (Software Directive Art. 6(3); CPI L122-6-1 V).
- **The account holder's own data only.** The agenda and client records are the account's own business
  records; reading them is the controller reading its own processing, and the sui generis right over data
  one created oneself is at its weakest.
- **An independent client.** It is an independently created program that interoperates; it does not
  reproduce the platform's program code and does not develop a *"substantially similar"* program (the
  limit in Software Directive Art. 6(2) / CPI L122-6-1 IV).
- **No circumvention of authentication or security.** The client defeats no TPM and no access control
  (keeping it clear of both the *Nintendo* TPM debate and the *Bluetouff* "fraudulent access/maintien"
  zone).
- **No access to any other account, and no scraping of the public booking graph.** No third party's data
  and no third party's authentication are ever touched.
- **No redistribution of the platform's code or proprietary interoperability information** beyond what is
  necessary for interoperability (Software Directive Art. 6(2); CPI L122-6-1 IV).

---

## Sources

Primary and authoritative texts, by their canonical EUR-Lex, Légifrance and CURIA permalinks:

**EU legislation**
- Directive 2009/24/EC (computer programs): https://eur-lex.europa.eu/eli/dir/2009/24/oj
- Directive 96/9/EC (databases): https://eur-lex.europa.eu/eli/dir/1996/9/oj
- Directive (EU) 2016/943 (trade secrets): https://eur-lex.europa.eu/eli/dir/2016/943/oj
- Regulation (EU) 2016/679 (GDPR): https://eur-lex.europa.eu/eli/reg/2016/679/oj
- Regulation (EU) 2023/2854 (Data Act): https://eur-lex.europa.eu/eli/reg/2023/2854/oj
- Directive 93/13/EEC (unfair terms): https://eur-lex.europa.eu/eli/dir/1993/13/oj

**French legislation**
- Code de la propriété intellectuelle, Art. L122-6-1 (via Légifrance): https://www.legifrance.gouv.fr/codes/texte_lc/LEGITEXT000006069414/
- Code pénal, Arts. 323-1 to 323-3: https://www.legifrance.gouv.fr/codes/id/LEGISCTA000006149839/

**CJEU judgments** (CURIA)
- C-406/10 SAS Institute v World Programming: https://curia.europa.eu/juris/documents.jsf?num=C-406/10
- C-159/23 Sony v Datel: https://curia.europa.eu/juris/documents.jsf?num=C-159/23
- C-128/11 UsedSoft v Oracle: https://curia.europa.eu/juris/documents.jsf?num=C-128/11
- C-355/12 Nintendo v PC Box: https://curia.europa.eu/juris/documents.jsf?num=C-355/12
- C-30/14 Ryanair v PR Aviation: https://curia.europa.eu/juris/documents.jsf?num=C-30/14

**United States (context only)**
- Van Buren v. United States, 593 U.S. 374 (2021): https://www.law.cornell.edu/supremecourt/text/19-783

---

## Disclaimer

This document is a **documented legal basis** drawn from public legal sources. It is **not legal
advice**, it creates no lawyer-client relationship, and it is not a guarantee that any particular act
is lawful. It addresses **European Union and French law only**; the position may differ in other
jurisdictions. The application of these provisions to any concrete situation is fact-dependent and is
ultimately a matter for a court. Anyone relying on this analysis for a real decision should have it
reviewed by a qualified French intellectual-property / IT lawyer against the current consolidated texts
and the latest case law.
