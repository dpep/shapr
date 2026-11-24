class A
  def go(a)
    show(a)
    1
  end

  def show(a)
    t = a * 2 + 1
    puts t.to_s + 'zz' + __FILE__
  end
end
