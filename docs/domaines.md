# Domaines contactés par MailALM

MailALM ne contacte que les serveurs des comptes configurés (PLAN.md, section 10.3). Pas de télémétrie, pas de
mise à jour automatique, pas de rapport de plantage, pas d'autoconfiguration par un service tiers.

## Phase 2 : comptes IMAP

| Domaine | Port | Protocole | Quand |
|---|---|---|---|
| `imap.videotron.ca` | 993 (TLS) | IMAP | Compte Vidéotron : lecture, dossiers, changements |
| `smtp.videotron.ca` | 465 (TLS) | SMTP | Compte Vidéotron : envoi |
| `imap.bell.net` | 993 (TLS) | IMAP | Compte Bell : lecture, dossiers, changements |
| `smtphm.sympatico.ca` | 587 (STARTTLS exigé) | SMTP | Compte Bell : envoi |
| serveurs saisis par l'utilisateur | ceux saisis | IMAP, SMTP | Autre compte IMAP |

- Les profils Vidéotron et Bell sont **à confirmer** : les pages d'aide des fournisseurs et les guides tiers ne
  s'accordent pas sur les serveurs d'envoi. Ils sont affichés et modifiables dans la fenêtre d'ajout de compte.
- Toutes les connexions sont chiffrées (TLS, ou STARTTLS exigé). Il n'existe pas d'option sans chiffrement ni
  d'exception de certificat.
- La vérification des certificats utilise les autorités de Windows et refuse les certificats que Windows désigne
  comme non fiables (magasin `Disallowed`).

## Phases suivantes

Microsoft Graph et la connexion Microsoft (phase 3), les API Google et la connexion Google (phase 4) : la liste
sera complétée à leur arrivée et validée par l'utilisateur.
