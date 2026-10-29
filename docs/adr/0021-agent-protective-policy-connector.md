# ADR-0021 — agent-protective policy without core content interpretation

Status: accepted  
Date: 2026-10  
Depends on: `docs/PLAN.md` (Fase 1..5 deste plano)

## Decisão

O core não avança para interpretar conteúdo, categorizar terceiros, decidir
consentimento ou aplicar proteção automática por natureza do dado.

Esse ADR define uma zona de responsabilidade explícita e um conjunto mínimo de
extensões de política operadasno connector/host, para que a memória do
`neural-sgdb` possa ser usada em contextos onde há terceiros, dados sensíveis e
restrições de retenção, sem que o core se torne juiz de conteúdo.

## Responsabilidade explícita

- Core: armazenamento, escopo, entidades, recall, curadoria e rastreabilidade
  estrutural da memória.
- Host/connector: conteúdo, classificação de terceiros, consentimento,
  retenção, acesso cross-scope e auditoria ligada ao domínio do produto.
- Product use-case: decidir quando o sistema é insuficiente sozinho, principalmente
  para memória sobre menores, contexto parental, dados de saúde, educação ou
  comportamento de terceiros.

Essa divisão não é detalhe de implementação. Ela deve aparecer no README, no guia
de integração e no contrato do connector.

## Extensões mínimas de política

O connector pode repassar metadados opcionais de política, por exemplo:

- `memory_category`
- `subject_type`
- `requires_consent`
- `retention_rule`

Eles devem ser repassados e protegidos pelo connector/host, não interpretados pelo
core como conteúdo. O objetivo é permitir que o host filtre, expirar, retenha ou
audite por categoria sem alterar o modelo do core.

## Controle de recall sensível

A `auto_recall_context` deve aceitar contexto de recall explícito, por exemplo:

- somente memórias do próprio escopo do agente
- somente memórias marcadas como “próprias”

O host deve bloquear recall que misture escopos ou sujeitos não autorizados.
Quando recall traz conteúdo sensível e o host está operando em contexto de
vulnerável, o connector/host deve emitir aviso explícito e não tratar a memória
como contexto neutro.

## Rastreabilidade e incidentes

O connector/host deve registrar, de forma leve, quando memória sobre terceiro é
escrita, sempre que o metadado de categoria indicar isso. O objetivo é auditoria
e remoção eventual, não vigilância automática.

`forget` e remoção de escopo devem ser usados também quando:

- o consentimento é revogado
- o prazo de retenção se esgota

Essa é uma obrigação de operação do host, não uma promessa automática do core.

## Scenario de teste

Deve existir teste host-side que simule:

- escrita de memória sobre terceiro com categoria sensível
- recall indevido
- tentativa de recall fora de escopo

E verifique que o connector/host rejeita ou filtra como esperado. Isso evita que
mudanças futuras reintroduzam recall cross-scope acidental.

## Limites

- Isso não torna o sistema conformity-ready por si só.
- O core não avalia conteúdo e não decide sobre terceiros.
- O host é o dono da política de conteúdo, consentimento e retenção.

## Consequências

- Leitura e integração ganham seção explícita de limites e responsabilidades.
- Connector ganha metadados de política opcionais e controle de recall sensível.
- Testes ganham scenario de recall sensível e fora de escopo.
- Core não muda sua posição de substrate de memória.
