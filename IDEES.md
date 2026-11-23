# MailALM : idées pour plus tard

Suggestions proposées le 2026-11-22 et non retenues pour la première version (décision Q13 de
[PLAN.md](PLAN.md)). À réexaminer après la version 1.0.

## Sécurité

| ID | Idée | Notes |
|---|---|---|
| S1 | Analyse d'une pièce jointe par Microsoft Defender avant son ouverture | Lancement de l'analyseur de Windows sur le fichier enregistré, avant l'ouverture approuvée |
| S2 | Ouverture d'une pièce jointe douteuse dans Windows Sandbox | Seulement si la fonction Windows est activée |
| S3 | Alertes « premier courriel de cet expéditeur » et « nom affiché d'un contact, adresse inconnue » | Complète le bandeau DMARC déjà prévu |
| S5 | Verrouillage de MailALM par Windows Hello ou un NIP après inactivité | Utile surtout si la base locale reste non chiffrée |

## Agenda

| ID | Idée | Notes |
|---|---|---|
| A4 | Barre latérale « À venir » : rendez-vous et tâches du jour à côté du courrier | |
| A5 | Création d'un événement depuis un courriel (glisser le message sur l'agenda) | Titre, texte et participants repris du message |

## Courrier

| ID | Idée | Notes |
|---|---|---|
| C8 | Chiffrement et signature S/MIME et OpenPGP | Code de MailCraft (`smime`, `pgp`, `keys`) à auditer avant reprise ; clés privées à protéger par phrase de passe |

## IA et MCP

| ID | Idée | Notes |
|---|---|---|
| M1 | Principe : pas d'IA intégrée qui contacte un service ; l'IA passe seulement par le MCP | Non posé comme question ; aujourd'hui de fait, aucun service d'IA n'est prévu |
