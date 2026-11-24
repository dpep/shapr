class A
  def go(a)
    show(a)
    binding.local_variables
  end

  def show(a)
    t = a * 2 + 1
    u = t.to_s + 'zz'
    puts u
  end
end
