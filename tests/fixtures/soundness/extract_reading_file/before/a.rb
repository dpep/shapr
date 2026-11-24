class A
  def go(a)
    t = a * 2 + 1
    puts t.to_s + 'zz' + __FILE__
    1
  end
end
