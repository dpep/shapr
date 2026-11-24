class A
  def go(a)
    show(a)
    puts $1
  end

  def show(a)
    a =~ /(\d+)/
    puts a.to_s + 'zz' + a.to_s
  end
end
