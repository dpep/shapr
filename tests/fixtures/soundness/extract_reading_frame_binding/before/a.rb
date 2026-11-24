class A
  def go(a)
    t = a * 2 + 1
    puts binding.local_variables.to_s + t.to_s
    1
  end
end
