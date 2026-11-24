class A
  def go(a)
    show(a)
    1
  end

  def show(a)
    t = a * 2 + 1
    puts binding.local_variables.to_s + t.to_s
  end
end
