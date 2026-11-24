class A
  def go(a)
    t = a * 2 + 1
    u = t.to_s + 'zz'
    puts u
    binding.local_variables
  end
end
