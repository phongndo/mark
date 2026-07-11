; x86-64 asm basic café
global _start
section .text
_start:
  mov rax, 60
  xor rdi, rdi
  syscall
