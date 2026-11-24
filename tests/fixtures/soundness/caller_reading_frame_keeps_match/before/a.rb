class A
  def go(a)
    a =~ /(\d+)/
    puts a.to_s + 'zz' + a.to_s
    puts $1
  end
end
